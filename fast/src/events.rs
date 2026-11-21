//! Events (rules.md "Events, in this order").
//!
//! Phase 1 parses every line into an `EvRec`, then writes it as two streams: an `EvCopy` (event_id,
//! ingest time) into a partition by `hash(event_id)`, and, if the line is valid and in time, a
//! `PRec` (person, ts, purchase) into a partition by `hash(person)`. Every well-formed line with a
//! non-empty `event_id` enters dedup, valid or not; whether it is valid (step 3) and not late
//! (step 4) is decided here, per line, and only matters if this copy is the one dedup keeps.

use crate::dedup::PRec;
use crate::key::{key_of, mix};
use crate::links::{LinkTable, AHEAD};
use crate::lookups::{Products, Window};
use crate::scan::{for_each_line, LineStats};
use crate::ts::{DayMemo, MS_PER_SECOND};
use crate::wc::{Lines, Wc};

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

/// One parsed event line (32 bytes).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct EvRec {
    pub eid: u64,
    /// The person key, resolved while parsing (valid events only, else 0).
    pub user: u64,
    /// `(ingest_seconds + 2^39) << 24 | category rank`. Dedup compares `ing_cat >> 24`.
    pub ing_cat: u64,
    /// `ts - window.base` in ms (valid events only).
    pub ts: u32,
    /// `VALID | PURCHASE | price` (price in the low 24 bits; at most 10,000,000).
    pub pay: u32,
}

pub const VALID: u32 = 1 << 31;
pub const PURCHASE: u32 = 1 << 30;
pub const PRICE_MASK: u32 = (1 << 24) - 1;
const LATE_MS: i64 = 86_400_000;
const ING_BIAS: i64 = 1 << 39;
const ING_MAX: i64 = (1 << 40) - 1;

impl EvRec {
    #[inline(always)]
    pub fn ingest(&self) -> u64 {
        self.ing_cat >> 24
    }
    #[inline(always)]
    pub fn category(&self) -> u32 {
        (self.ing_cat & 0xFF_FFFF) as u32
    }
    #[inline(always)]
    pub fn valid(&self) -> bool {
        self.pay & VALID != 0
    }
}

/// `price_cents` of a purchase: ASCII digits only, 1..=10,000,000, leading zeros allowed.
#[inline(always)]
pub fn parse_price(b: &[u8]) -> Option<u32> {
    if b.is_empty() {
        return None;
    }
    let mut value: u32 = 0;
    let mut significant = 0;
    for &c in b {
        let d = c.wrapping_sub(b'0');
        if d >= 10 {
            return None;
        }
        if value != 0 || d != 0 {
            significant += 1;
            if significant > 8 {
                return None; // at least 100,000,000: too large (and stops overflow)
            }
            value = value * 10 + d as u32;
        }
    }
    if (1..=10_000_000).contains(&value) { Some(value) } else { None }
}

pub struct EventParser<'a> {
    pub window: Window,
    pub products: &'a Products,
}

impl EventParser<'_> {
    /// The record for one well-formed line, or `None` if its `event_id` is empty (dropped at parse).
    #[inline(always)]
    pub fn parse(&self, f: &[&[u8]; 7], memo: &mut DayMemo) -> Option<EvRec> {
        if f[0].is_empty() {
            return None;
        }
        let w = &self.window;
        let ingest = memo.parse_ingest_seconds(f[3]);
        let ing = match ingest {
            Some(s) => (s + ING_BIAS).clamp(0, ING_MAX) as u64,
            None => ING_MAX as u64,
        };
        let mut valid = ingest.is_some() & !f[1].is_empty() & !f[5].is_empty();
        let ts_ms = match memo.parse_ts(f[2]) {
            Some(t) if t >= w.start && t < w.end => t,
            _ => {
                valid = false;
                0
            }
        };
        // Event type by length and one integer compare (no memcmp calls).
        let ty = f[4];
        let (known, purchase) = match ty.len() {
            4 => {
                let w = u32::from_le_bytes(ty[..4].try_into().unwrap());
                (w == u32::from_le_bytes(*b"view") || w == u32::from_le_bytes(*b"cart"), false)
            }
            8 => {
                let p = u64::from_le_bytes(ty[..8].try_into().unwrap()) == u64::from_le_bytes(*b"purchase");
                (p, p)
            }
            _ => (false, false),
        };
        if !known {
            valid = false;
        }
        let mut price = 0u32;
        let mut category = 0u32;
        if purchase && valid {
            match parse_price(f[6]) {
                Some(p) => {
                    price = p;
                    category = self.products.category(key_of(f[5]));
                }
                None => valid = false,
            }
        }
        if valid {
            // Late: ingest_ts - ts over 24 h, compared in ms. Exactly 24 h is kept.
            if let Some(s) = ingest {
                if s * MS_PER_SECOND - ts_ms > LATE_MS {
                    valid = false;
                }
            }
        }
        let (user, ts) = if valid { (key_of(f[1]), (ts_ms - w.base) as u32) } else { (0, 0) };
        Some(EvRec {
            eid: key_of(f[0]),
            user,
            ing_cat: (ing << 24) | category as u64,
            ts,
            pay: ((valid as u32) << 31) | ((purchase as u32) << 30) | price,
        })
    }
}

#[derive(Default, Clone, Copy, Debug)]
pub struct ParseStats {
    pub lines: LineStats,
    /// Well-formed lines with an empty event_id (or click_id).
    pub empty_id: u64,
    /// Records produced.
    pub records: u64,
    /// Records that are valid on their own.
    pub valid: u64,
}

impl ParseStats {
    pub fn add(&mut self, o: ParseStats) {
        self.lines.add(o.lines);
        self.empty_id += o.empty_id;
        self.records += o.records;
        self.valid += o.valid;
    }
}

/// One copy of an event, in its `event_id` partition (16 bytes): all that dedup needs.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct EvCopy {
    pub eid: u64,
    /// `(ingest_seconds + 2^39) << 24 | at`: `at` is the index of this copy's `PRec` in its chunk's
    /// `events`, or `NO_EVENT` if the copy is invalid or late (it can still win dedup).
    pub ing_at: u64,
}

pub const NO_EVENT: u64 = (1 << 24) - 1;

impl EvCopy {
    #[inline(always)]
    pub fn ingest(&self) -> u64 {
        self.ing_at >> 24
    }
    #[inline(always)]
    pub fn at(&self) -> u64 {
        self.ing_at & NO_EVENT
    }
}

/// One chunk, as two counting-sorted streams. `copies`: every copy with an `event_id`, by P1
/// partition (`hash(event_id)`). `events` (returned beside it, so dedup can mark it while the
/// copies are shared): one `PRec` per valid, in-time copy, by P2 partition (`hash(person)`).
/// Dedup marks the `PRec` of every copy that loses; phase 3 reads the rest.
pub struct EvChunk {
    pub copies: Lines<EvCopy>,
    pub coffs: Vec<u32>,
    pub eoffs: Vec<u32>,
    /// (price, category rank) of this chunk's valid purchases; `PRec::pay` is index + 1.
    pub purchases: Vec<(u32, u32)>,
}

impl EvChunk {
    #[inline(always)]
    pub fn copies(&self, p: usize) -> &[EvCopy] {
        &self.copies[self.coffs[p] as usize..self.coffs[p + 1] as usize]
    }
}

thread_local! {
    static TMP: std::cell::RefCell<(Vec<EvRec>, Vec<u16>, Vec<u16>)> =
        const { std::cell::RefCell::new((Vec::new(), Vec::new(), Vec::new())) };
    static WC: std::cell::RefCell<(Wc<EvCopy>, Wc<PRec>)> = const { std::cell::RefCell::new((Wc::new(), Wc::new())) };
}

/// Scatter with plain stores instead of write-combining (`FAST_NO_WC=1`, for A/B runs).
pub static PLAIN_SCATTER: AtomicBool = AtomicBool::new(false);

/// Turns per-partition counts (at `i + 1`) into start offsets.
fn prefix(counts: &mut [u32]) {
    for i in 1..counts.len() {
        counts[i] += counts[i - 1];
    }
}

/// Parses one chunk into its two streams. Each valid record's user is replaced by its person: the
/// link-table slot is prefetched when the line is parsed and read `AHEAD` lines later, so the
/// lookups overlap the parsing instead of stalling on memory.
#[allow(clippy::too_many_arguments)]
pub fn parse_chunk(parser: &EventParser, links: &LinkTable, data: &[u8], start: usize, end: usize, p1: usize, shift1: u32, p2: usize, shift2: u32) -> (EvChunk, Lines<PRec>, ParseStats) {
    TMP.with(|cell| {
        let mut guard = cell.borrow_mut();
        let (tmp, ps, qs) = &mut *guard;
        tmp.clear();
        ps.clear();
        qs.clear();
        let mut coffs = vec![0u32; p1 + 1];
        let mut eoffs = vec![0u32; p2 + 1];
        // Resolves record `r` to its person and notes its P2 partition (records resolve in order,
        // so `qs` stays aligned with `tmp`).
        let resolve = |r: &mut EvRec, qs: &mut Vec<u16>, eoffs: &mut [u32]| {
            if r.valid() {
                r.user = links.person(r.user);
                let q = (mix(r.user) >> shift2) as usize;
                eoffs[q + 1] += 1;
                qs.push(q as u16);
            } else {
                qs.push(0);
            }
        };
        let mut st = ParseStats::default();
        let mut memo = DayMemo::default();
        st.lines = for_each_line::<7, _>(data, start, end, |f| match parser.parse(f, &mut memo) {
            Some(rec) => {
                st.records += 1;
                st.valid += rec.valid() as u64;
                let p = (mix(rec.eid) >> shift1) as usize;
                coffs[p + 1] += 1;
                if rec.valid() {
                    links.prefetch(rec.user);
                }
                tmp.push(rec);
                ps.push(p as u16);
                let n = tmp.len();
                if n > AHEAD {
                    resolve(&mut tmp[n - 1 - AHEAD], qs, &mut eoffs);
                }
            }
            None => st.empty_id += 1,
        });
        let n = tmp.len();
        for r in tmp[n.saturating_sub(AHEAD)..].iter_mut() {
            resolve(r, qs, &mut eoffs);
        }
        assert!(st.valid < NO_EVENT, "chunk too large");
        prefix(&mut coffs);
        prefix(&mut eoffs);
        let nv = eoffs[p2] as usize;
        let mut copies: Lines<EvCopy> = Lines::new(n);
        let mut events: Lines<PRec> = Lines::new(nv);
        let mut purchases: Vec<(u32, u32)> = Vec::new();
        let (cdst, edst) = (copies.as_mut_ptr(), events.as_mut_ptr());
        let mut pay_of = |r: &EvRec| {
            if r.pay & PURCHASE != 0 {
                purchases.push((r.pay & PRICE_MASK, r.category()));
                purchases.len() as u32
            } else {
                0
            }
        };
        let recs = tmp.iter().zip(ps.iter()).zip(qs.iter());
        if !PLAIN_SCATTER.load(Relaxed) {
            WC.with(|wc| {
                let (wcc, wce) = &mut *wc.borrow_mut();
                wcc.reset(&coffs);
                wce.reset(&eoffs);
                for ((r, &p), &q) in recs {
                    // SAFETY: the offsets come from counting these very records, so each slot of
                    // both outputs is put exactly once, and every partition index is in range.
                    unsafe {
                        let at = if r.valid() { wce.put(edst, q as usize, PRec { person: r.user, ts: r.ts, pay: pay_of(r) }) as u64 } else { NO_EVENT };
                        wcc.put(cdst, p as usize, EvCopy { eid: r.eid, ing_at: (r.ingest() << 24) | at });
                    }
                }
                // SAFETY: the destinations of the puts above.
                unsafe {
                    wcc.finish(cdst);
                    wce.finish(edst);
                }
            });
        } else {
            let mut ccur: Vec<u32> = coffs[..p1].to_vec();
            let mut ecur: Vec<u32> = eoffs[..p2].to_vec();
            let (cdst, edst) = (cdst as *mut EvCopy, edst as *mut PRec);
            for ((r, &p), &q) in recs {
                let at = if r.valid() {
                    let at = ecur[q as usize];
                    ecur[q as usize] += 1;
                    // SAFETY: the cursors of all P2 partitions together cover 0..nv exactly once.
                    unsafe { edst.add(at as usize).write(PRec { person: r.user, ts: r.ts, pay: pay_of(r) }) };
                    at as u64
                } else {
                    NO_EVENT
                };
                let c = ccur[p as usize];
                ccur[p as usize] += 1;
                // SAFETY: the cursors of all P1 partitions together cover 0..n exactly once.
                unsafe { cdst.add(c as usize).write(EvCopy { eid: r.eid, ing_at: (r.ingest() << 24) | at }) };
            }
        }
        // SAFETY: every slot of both was written above.
        unsafe {
            copies.written();
            events.written();
        }
        (EvChunk { copies, coffs, eoffs, purchases }, events, st)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices() {
        for (s, want) in [
            ("1", Some(1)),
            ("0250", Some(250)),
            ("10000000", Some(10_000_000)),
            ("000000000000000000010000000", Some(10_000_000)),
            ("10000001", None),
            ("250000000", None),
            ("0", None),
            ("00", None),
            ("", None),
            ("19.99", None),
            ("-5", None),
            ("+500", None),
            ("1e3", None),
            ("1000\r", None),
            (" 100", None),
            ("\u{FF12}\u{FF15}\u{FF10}", None),
            ("\u{0662}\u{0665}\u{0660}", None),
            ("99999999999999999999999999", None),
        ] {
            assert_eq!(parse_price(s.as_bytes()), want, "{s:?}");
        }
    }
}
