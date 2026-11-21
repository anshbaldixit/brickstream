//! Phase 2: event dedup and the move to person partitions.
//!
//! P1 partition `p` (top bits of `hash(event_id)`) holds every copy of its event IDs, one slice per
//! parsing thread, so dedup is local: an open-addressing table keeps, per `event_id`, the copy with the
//! smallest `ingest_ts` (rules.md Events step 2; equal ingest times mean identical copies, either may
//! stay). Other copies are marked in a loser bitmap. The kept copy is dropped if it is invalid or late
//! (steps 3-4, decided per line in phase 1). Survivors get their person from the link table and go,
//! through the thread's write-combining partitioner, to P2 partition `top bits of hash(person)` as
//! 16-byte `PRec`s; a purchase's price and category go to the thread's side table. Clicks are resolved
//! and partitioned the same way.

use std::cell::RefCell;
use std::time::Instant;

use crate::clicks::ClickRec;
use crate::events::{EvRec, PRICE_MASK, PURCHASE};
use crate::key::mix;
use crate::links::LinkTable;

/// One surviving event in a person partition.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct PRec {
    pub person: u64,
    pub ts: u32,
    /// 0: not a purchase. Otherwise `(thread << SIDE_BITS | index) + 1` into that thread's `purchases`.
    pub pay: u32,
}

/// Bits of the purchase index in `PRec::pay` (the P1 partition takes the bits above).
pub const SIDE_BITS: u32 = 19;

/// The survivors of one P1 partition, counting-sorted by P2 partition.
pub struct P2Part {
    pub recs: Vec<PRec>,
    pub offs: Vec<u32>,
    /// (price, category rank) of its purchases.
    pub purchases: Vec<(u32, u32)>,
}

impl P2Part {
    #[inline(always)]
    pub fn part(&self, q: usize) -> &[PRec] {
        &self.recs[self.offs[q] as usize..self.offs[q + 1] as usize]
    }
}

/// One click chunk, resolved to persons and counting-sorted by P2 partition.
pub struct ClChunk {
    pub recs: Vec<ClickRec>,
    pub offs: Vec<u32>,
}

impl ClChunk {
    #[inline(always)]
    pub fn part(&self, q: usize) -> &[ClickRec] {
        &self.recs[self.offs[q] as usize..self.offs[q + 1] as usize]
    }
}

/// Counting sort of `items` by `qs` into a new buffer, with partition offsets.
fn counting_sort<T: Copy>(items: &[T], qs: &[u16], p2: usize) -> (Vec<T>, Vec<u32>) {
    let mut offs = vec![0u32; p2 + 1];
    for &q in qs {
        offs[q as usize + 1] += 1;
    }
    for i in 0..p2 {
        offs[i + 1] += offs[i];
    }
    let mut cursor: Vec<u32> = offs[..p2].to_vec();
    let n = items.len();
    let mut out: Vec<T> = Vec::with_capacity(n);
    let dst = out.as_mut_ptr();
    for (item, &q) in items.iter().zip(qs) {
        let at = cursor[q as usize] as usize;
        cursor[q as usize] += 1;
        // SAFETY: the cursors together cover 0..n exactly once.
        unsafe { dst.add(at).write(*item) };
    }
    // SAFETY: all n slots were written.
    unsafe { out.set_len(n) };
    (out, offs)
}

#[derive(Default)]
struct Scratch {
    /// Dedup table slots: `fingerprint << 32 | (index + 1)`, 0 = empty.
    slots: Vec<u64>,
    losers: Vec<u64>,
    starts: Vec<usize>,
    surv: Vec<PRec>,
    qs: Vec<u16>,
}

thread_local! {
    static SCRATCH: RefCell<Scratch> = RefCell::new(Scratch::default());
}

#[derive(Default, Clone, Copy, Debug)]
pub struct DedupStats {
    pub records: u64,
    pub losers: u64,
    pub kept_invalid: u64,
    pub survivors: u64,
    pub purchases: u64,
    pub ns_insert: u64,
    pub ns_resolve: u64,
    pub ns_scatter: u64,
}

impl DedupStats {
    pub fn add(&mut self, o: DedupStats) {
        self.records += o.records;
        self.losers += o.losers;
        self.kept_invalid += o.kept_invalid;
        self.survivors += o.survivors;
        self.purchases += o.purchases;
        self.ns_insert += o.ns_insert;
        self.ns_resolve += o.ns_resolve;
        self.ns_scatter += o.ns_scatter;
    }
}

const BATCH: usize = 256;

/// Dedups P1 partition `p` (one slice per chunk) and returns its survivors by P2 partition.
pub fn partition(p: usize, slices: &[&[EvRec]], links: &LinkTable, p2: usize, shift2: u32) -> (P2Part, DedupStats) {
    let mut st = DedupStats::default();
    let out = SCRATCH.with(|cell| {
        let mut guard = cell.borrow_mut();
        let sc = &mut *guard;
        sc.starts.clear();
        let mut n = 0usize;
        for s in slices {
            sc.starts.push(n);
            n += s.len();
        }
        st.records = n as u64;
        let t0 = Instant::now();
        // 8-byte slots (half the size of key + index) keep the table in L2. A fingerprint match is
        // confirmed against the record itself, so a collision can never merge two event IDs.
        let cap = (2 * n).next_power_of_two().max(16);
        let mask = cap - 1;
        sc.slots.clear();
        sc.slots.resize(cap, 0);
        sc.losers.clear();
        sc.losers.resize(n.div_ceil(64) + 1, 0);
        let starts = &sc.starts;
        let locate = |w: usize| -> &EvRec {
            let s = starts.partition_point(|&st| st <= w) - 1;
            &slices[s][w - starts[s]]
        };
        for (s, slice) in slices.iter().enumerate() {
            let base = starts[s];
            for (j, rec) in slice.iter().enumerate() {
                let i = base + j;
                let hm = mix(rec.eid);
                let fp = hm >> 32;
                let mut h = hm as usize & mask;
                loop {
                    let v = sc.slots[h];
                    if v == 0 {
                        sc.slots[h] = (fp << 32) | (i as u64 + 1);
                        break;
                    }
                    if v >> 32 == fp {
                        let w = (v as u32 - 1) as usize;
                        let other = locate(w);
                        if other.eid == rec.eid {
                            let lose = if rec.ingest() < other.ingest() {
                                sc.slots[h] = (fp << 32) | (i as u64 + 1);
                                w
                            } else {
                                i
                            };
                            sc.losers[lose / 64] |= 1 << (lose % 64);
                            st.losers += 1;
                            break;
                        }
                    }
                    h = (h + 1) & mask;
                }
            }
        }
        st.ns_insert += t0.elapsed().as_nanos() as u64;

        // The kept copies that are valid, resolved in batches so link lookups can be prefetched.
        sc.surv.clear();
        sc.qs.clear();
        let mut purchases: Vec<(u32, u32)> = Vec::new();
        let mut people = [0u64; BATCH];
        let mut recs: [&EvRec; BATCH] = [&DUMMY; BATCH];
        let mut fill = 0;
        let mut flush = |people: &mut [u64; BATCH], recs: &[&EvRec; BATCH], fill: usize, sc: &mut Scratch, st: &mut DedupStats| {
            let t1 = Instant::now();
            links.resolve(&mut people[..fill]);
            let t2 = Instant::now();
            st.ns_resolve += (t2 - t1).as_nanos() as u64;
            for b in 0..fill {
                let r = recs[b];
                let person = people[b];
                let pay = if r.pay & PURCHASE != 0 {
                    let idx = purchases.len() as u32;
                    assert!(idx >> SIDE_BITS == 0, "too many purchases in one partition");
                    purchases.push((r.pay & PRICE_MASK, r.category()));
                    (((p as u32) << SIDE_BITS) | idx) + 1
                } else {
                    0
                };
                sc.surv.push(PRec { person, ts: r.ts, pay });
                sc.qs.push((mix(person) >> shift2) as u16);
            }
            st.survivors += fill as u64;
            st.ns_scatter += t2.elapsed().as_nanos() as u64;
        };
        for (s, slice) in slices.iter().enumerate() {
            let base = sc.starts[s];
            for (j, rec) in slice.iter().enumerate() {
                let i = base + j;
                if sc.losers[i / 64] >> (i % 64) & 1 != 0 {
                    continue;
                }
                if !rec.valid() {
                    st.kept_invalid += 1;
                    continue;
                }
                people[fill] = rec.user;
                recs[fill] = rec;
                fill += 1;
                if fill == BATCH {
                    flush(&mut people, &recs, fill, sc, &mut st);
                    fill = 0;
                }
            }
        }
        flush(&mut people, &recs, fill, sc, &mut st);
        st.purchases = purchases.len() as u64;
        let t3 = Instant::now();
        let (recs, offs) = counting_sort(&sc.surv, &sc.qs, p2);
        st.ns_scatter += t3.elapsed().as_nanos() as u64;
        P2Part { recs, offs, purchases }
    });
    (out, st)
}

static DUMMY: EvRec = EvRec { eid: 0, user: 0, ing_cat: 0, ts: 0, pay: 0 };

/// Resolves one chunk's valid clicks to persons and counting-sorts them by P2 partition.
pub fn clicks(list: &mut [ClickRec], links: &LinkTable, p2: usize, shift2: u32) -> ClChunk {
    let mut people = [0u64; BATCH];
    let mut qs: Vec<u16> = Vec::with_capacity(list.len());
    for block in list.chunks_mut(BATCH) {
        for (p, c) in people.iter_mut().zip(block.iter()) {
            *p = c.who;
        }
        links.resolve(&mut people[..block.len()]);
        for (c, &p) in block.iter_mut().zip(people.iter()) {
            c.who = p;
            qs.push((mix(p) >> shift2) as u16);
        }
    }
    let (recs, offs) = counting_sort(list, &qs, p2);
    ClChunk { recs, offs }
}
