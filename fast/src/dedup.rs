//! Phase 2: event dedup, in place.
//!
//! P1 partition `p` (top bits of `hash(event_id)`) holds every copy of its event IDs (one slice per
//! chunk), so dedup is local: an open-addressing table keeps, per `event_id`, the copy with the
//! smallest `ingest_ts` (rules.md Events step 2; equal ingest times mean identical copies, either may
//! stay). Every other copy loses, and if it has a `PRec` (it was valid and in time), that `PRec` is
//! marked `DROPPED` where it lies in its person partition. A kept copy that is invalid or late has no
//! `PRec` (steps 3-4, decided per line in phase 1), so its event is gone, whatever later copies say.

use std::cell::RefCell;
use std::time::Instant;

use crate::clicks::ClickRec;
use crate::events::{EvChunk, EvCopy, NO_EVENT};
use crate::key::mix;

/// One valid, in-time event in a person partition.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct PRec {
    pub person: u64,
    /// `ts - window.base` in ms, or `DROPPED` once dedup has given the event to another copy.
    pub ts: u32,
    /// 0: not a purchase. Otherwise index + 1 into its chunk's `purchases`.
    pub pay: u32,
}

/// `PRec::ts` of a copy that lost dedup. Never a real offset: the window check keeps every offset
/// below `u32::MAX`.
pub const DROPPED: u32 = u32::MAX;

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
pub fn counting_sort<T: Copy>(items: &[T], qs: &[u16], p2: usize) -> (Vec<T>, Vec<u32>) {
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

/// Write access to every chunk's `PRec`s while dedup runs. Each `PRec` belongs to one copy, and
/// each copy to one P1 partition, so no two dedup tasks ever write the same `PRec`, and nothing
/// reads them until dedup is over.
pub struct Marks(Vec<*mut PRec>);

// SAFETY: see above; the pointers come from `&mut` borrows that end only after dedup.
unsafe impl Sync for Marks {}
unsafe impl Send for Marks {}

impl Marks {
    pub fn new(events: &mut [Vec<PRec>]) -> Marks {
        Marks(events.iter_mut().map(|v| v.as_mut_ptr()).collect())
    }

    #[inline(always)]
    fn drop_event(&self, chunk: usize, at: u64) {
        // SAFETY: `at` indexes a written PRec of chunk `chunk` (from its EvCopy), and only this
        // copy's dedup task writes it.
        unsafe { (*self.0[chunk].add(at as usize)).ts = DROPPED };
    }
}

#[derive(Default)]
struct Scratch {
    /// Dedup table slots: `fingerprint << 32 | (index + 1)`, 0 = empty.
    slots: Vec<u64>,
    starts: Vec<usize>,
}

thread_local! {
    static SCRATCH: RefCell<Scratch> = RefCell::new(Scratch::default());
}

#[derive(Default, Clone, Copy, Debug)]
pub struct DedupStats {
    /// Copies (well-formed lines with an event_id).
    pub records: u64,
    pub losers: u64,
    /// Kept copies that are invalid or late.
    pub kept_invalid: u64,
    /// Kept copies that are valid: the events.
    pub survivors: u64,
    pub ns: u64,
}

impl DedupStats {
    pub fn add(&mut self, o: DedupStats) {
        self.records += o.records;
        self.losers += o.losers;
        self.kept_invalid += o.kept_invalid;
        self.survivors += o.survivors;
        self.ns += o.ns;
    }
}

/// Dedups the P1 partitions `ps`, marking every losing copy's `PRec` as `DROPPED`.
pub fn group(ps: std::ops::Range<usize>, chunks: &[EvChunk], marks: &Marks) -> DedupStats {
    let t = Instant::now();
    let mut st = DedupStats::default();
    let mut slices: Vec<(usize, &[EvCopy])> = Vec::with_capacity(chunks.len());
    SCRATCH.with(|cell| {
        let mut guard = cell.borrow_mut();
        let sc = &mut *guard;
        for p in ps {
            slices.clear();
            slices.extend(chunks.iter().enumerate().map(|(c, ch)| (c, ch.copies(p))).filter(|(_, s)| !s.is_empty()));
            one(&slices, sc, marks, &mut st);
        }
    });
    st.ns = t.elapsed().as_nanos() as u64;
    st
}

/// Dedups one P1 partition: `(chunk, copies)` slices.
fn one(slices: &[(usize, &[EvCopy])], sc: &mut Scratch, marks: &Marks, st: &mut DedupStats) {
    sc.starts.clear();
    let mut n = 0usize;
    for (_, s) in slices {
        sc.starts.push(n);
        n += s.len();
    }
    // 8-byte slots (half the size of key + index) keep the table in L2. A fingerprint match is
    // confirmed against the copy itself, so a collision can never merge two event IDs.
    let cap = (2 * n).next_power_of_two().max(16);
    let mask = cap - 1;
    sc.slots.clear();
    sc.slots.resize(cap, 0);
    let starts = &sc.starts;
    let locate = |w: usize| -> (usize, &EvCopy) {
        let s = starts.partition_point(|&x| x <= w) - 1;
        (slices[s].0, &slices[s].1[w - starts[s]])
    };
    let (mut losers, mut no_event, mut lost_no_event) = (0u64, 0u64, 0u64);
    for (s, &(chunk, slice)) in slices.iter().enumerate() {
        let base = starts[s];
        for (j, rec) in slice.iter().enumerate() {
            let i = base + j;
            no_event += (rec.at() == NO_EVENT) as u64;
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
                    let (ochunk, other) = locate((v as u32 - 1) as usize);
                    if other.eid == rec.eid {
                        // The smaller ingest time keeps the event; on a tie the first copy stays.
                        let (lchunk, lat) = if rec.ingest() < other.ingest() {
                            sc.slots[h] = (fp << 32) | (i as u64 + 1);
                            (ochunk, other.at())
                        } else {
                            (chunk, rec.at())
                        };
                        if lat == NO_EVENT {
                            lost_no_event += 1;
                        } else {
                            marks.drop_event(lchunk, lat);
                        }
                        losers += 1;
                        break;
                    }
                }
                h = (h + 1) & mask;
            }
        }
    }
    let kept_invalid = no_event - lost_no_event;
    st.records += n as u64;
    st.losers += losers;
    st.kept_invalid += kept_invalid;
    st.survivors += n as u64 - losers - kept_invalid;
}
