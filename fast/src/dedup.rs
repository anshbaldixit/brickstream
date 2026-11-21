//! Phase 2: event dedup and the move to person partitions.
//!
//! Each P1 partition holds every copy of its event IDs (P1 = top bits of `hash(event_id)`), so dedup
//! is local: an open-addressing table keeps, per `event_id`, the copy with the smallest `ingest_ts`
//! (rules.md Events step 2; equal ingest times mean identical copies, either may stay). The other
//! copies are marked in a loser bitmap. The kept copy is dropped if it is invalid or late (steps 3-4,
//! decided per line in phase 1). Survivors get their person from the link table and move to the P2
//! partition `top bits of hash(person)`, as 16-byte `PRec`s; a purchase's price and category go to
//! the thread's side table. Clicks are resolved and moved the same way.

use std::cell::RefCell;
use std::sync::Mutex;

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
    /// 0: not a purchase. Otherwise `(thread << 26 | index) + 1` into that thread's `purchases`.
    pub pay: u32,
}

pub const SIDE_BITS: u32 = 26;

/// What one thread wrote into the person partitions.
pub struct Out {
    pub events: Vec<Vec<PRec>>,
    pub clicks: Vec<Vec<ClickRec>>,
    /// (price, category) of purchases, indexed by `PRec::pay`.
    pub purchases: Vec<(u32, u32)>,
}

impl Out {
    pub fn new(p2: usize, ev_cap: usize, cl_cap: usize) -> Out {
        Out {
            events: (0..p2).map(|_| Vec::with_capacity(ev_cap)).collect(),
            clicks: (0..p2).map(|_| Vec::with_capacity(cl_cap)).collect(),
            purchases: Vec::new(),
        }
    }
}

#[derive(Default)]
struct Scratch {
    keys: Vec<u64>,
    vals: Vec<u32>,
    losers: Vec<u64>,
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
}

impl DedupStats {
    pub fn add(&mut self, o: DedupStats) {
        self.records += o.records;
        self.losers += o.losers;
        self.kept_invalid += o.kept_invalid;
        self.survivors += o.survivors;
        self.purchases += o.purchases;
    }
}

const BATCH: usize = 256;

/// Dedups P1 partition `p` (its slices from every thread) and moves survivors into `out`.
pub fn partition(slices: &[&[EvRec]], links: &LinkTable, out: &Mutex<Out>, tid: usize, shift2: u32) -> DedupStats {
    let mut st = DedupStats::default();
    let n: usize = slices.iter().map(|s| s.len()).sum();
    st.records = n as u64;
    if n == 0 {
        return st;
    }
    SCRATCH.with(|cell| {
        let mut guard = cell.borrow_mut();
        let sc = &mut *guard;
        let cap = (2 * n).next_power_of_two();
        let mask = cap - 1;
        sc.keys.clear();
        sc.keys.resize(cap, 0);
        sc.vals.resize(cap, 0);
        sc.losers.clear();
        sc.losers.resize(n.div_ceil(64), 0);

        // Global index i = offset of the slice + position; vals hold (slice << 24 | position).
        let mut offsets = [0usize; 33];
        for (s, slice) in slices.iter().enumerate() {
            offsets[s + 1] = offsets[s] + slice.len();
        }
        for (s, slice) in slices.iter().enumerate() {
            for (j, rec) in slice.iter().enumerate() {
                let i = offsets[s] + j;
                let mut h = mix(rec.eid) as usize & mask;
                loop {
                    let k = sc.keys[h];
                    if k == 0 {
                        sc.keys[h] = rec.eid;
                        sc.vals[h] = ((s as u32) << 24) | j as u32;
                        break;
                    }
                    if k == rec.eid {
                        let v = sc.vals[h];
                        let (ws, wj) = ((v >> 24) as usize, (v & 0xFF_FFFF) as usize);
                        let w = offsets[ws] + wj;
                        if rec.ingest() < slices[ws][wj].ingest() {
                            sc.losers[w / 64] |= 1 << (w % 64);
                            sc.vals[h] = ((s as u32) << 24) | j as u32;
                        } else {
                            sc.losers[i / 64] |= 1 << (i % 64);
                        }
                        st.losers += 1;
                        break;
                    }
                    h = (h + 1) & mask;
                }
            }
        }

        // Emit the kept copies that are valid, in batches so link lookups can be prefetched.
        let mut o = out.lock().unwrap();
        let o = &mut *o;
        let mut people = [0u64; BATCH];
        let mut recs: [&EvRec; BATCH] = [&DUMMY; BATCH];
        let mut fill = 0;
        let mut flush = |people: &mut [u64; BATCH], recs: &[&EvRec; BATCH], fill: usize, o: &mut Out, st: &mut DedupStats| {
            links.resolve(&mut people[..fill]);
            for b in 0..fill {
                let r = recs[b];
                let person = people[b];
                let pay = if r.pay & PURCHASE != 0 {
                    let idx = o.purchases.len() as u32;
                    o.purchases.push((r.pay & PRICE_MASK, r.category()));
                    st.purchases += 1;
                    (((tid as u32) << SIDE_BITS) | idx) + 1
                } else {
                    0
                };
                let q = (mix(person) >> shift2) as usize;
                o.events[q].push(PRec { person, ts: r.ts, pay });
            }
        };
        for (s, slice) in slices.iter().enumerate() {
            for (j, rec) in slice.iter().enumerate() {
                let i = offsets[s] + j;
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
                    flush(&mut people, &recs, fill, o, &mut st);
                    st.survivors += fill as u64;
                    fill = 0;
                }
            }
        }
        flush(&mut people, &recs, fill, o, &mut st);
        st.survivors += fill as u64;
    });
    st
}

static DUMMY: EvRec = EvRec { eid: 0, user: 0, ing_cat: 0, ts: 0, pay: 0 };

/// Resolves one chunk's valid clicks to persons and moves them into `out`.
pub fn clicks(list: &mut [ClickRec], links: &LinkTable, out: &Mutex<Out>, shift2: u32) {
    let mut o = out.lock().unwrap();
    let mut people = [0u64; BATCH];
    for block in list.chunks_mut(BATCH) {
        for (p, c) in people.iter_mut().zip(block.iter()) {
            *p = c.who;
        }
        links.resolve(&mut people[..block.len()]);
        for (c, &p) in block.iter_mut().zip(people.iter()) {
            c.who = p;
            let q = (mix(p) >> shift2) as usize;
            o.clicks[q].push(*c);
        }
    }
}
