//! Events (rules.md "Events, in this order").
//!
//! Phase 1 parses every line into an `EvRec` and drops it into a partition by `hash(event_id)`.
//! Every well-formed line with a non-empty `event_id` enters dedup, valid or not; whether it is valid
//! (step 3) and not late (step 4) is decided here, per line, and only matters if this copy is the one
//! dedup keeps. Phase 2 (dedup + resolve) lives below.

use crate::key::{key_of, mix};
use crate::lookups::{Products, Window};
use crate::scan::{for_each_line, LineStats};
use crate::ts::{parse_ingest_seconds, parse_ts, MS_PER_SECOND};

/// One parsed event line (32 bytes).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct EvRec {
    pub eid: u64,
    /// The user key (valid events only, else 0).
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
    pub fn parse(&self, f: &[&[u8]; 7]) -> Option<EvRec> {
        if f[0].is_empty() {
            return None;
        }
        let w = &self.window;
        let ingest = parse_ingest_seconds(f[3]);
        let ing = match ingest {
            Some(s) => (s + ING_BIAS).clamp(0, ING_MAX) as u64,
            None => ING_MAX as u64,
        };
        let mut valid = ingest.is_some() & !f[1].is_empty() & !f[5].is_empty();
        let ts_ms = match parse_ts(f[2]) {
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

/// One chunk's records, counting-sorted by P1 partition: partition `p` is `recs[offs[p]..offs[p + 1]]`.
pub struct EvChunk {
    pub recs: Vec<EvRec>,
    pub offs: Vec<u32>,
}

impl EvChunk {
    #[inline(always)]
    pub fn part(&self, p: usize) -> &[EvRec] {
        &self.recs[self.offs[p] as usize..self.offs[p + 1] as usize]
    }
}

thread_local! {
    static TMP: std::cell::RefCell<(Vec<EvRec>, Vec<u16>)> = const { std::cell::RefCell::new((Vec::new(), Vec::new())) };
}

/// Parses `data[start..end]`: records are written sequentially, then counting-sorted by the top bits
/// of `hash(event_id)` into the chunk's own buffer.
pub fn parse_chunk(parser: &EventParser, data: &[u8], start: usize, end: usize, p1: usize, shift: u32) -> (EvChunk, ParseStats) {
    TMP.with(|cell| {
        let mut guard = cell.borrow_mut();
        let (tmp, parts) = &mut *guard;
        tmp.clear();
        parts.clear();
        let mut offs = vec![0u32; p1 + 1];
        let mut st = ParseStats::default();
        st.lines = for_each_line::<7, _>(data, start, end, |f| match parser.parse(f) {
            Some(rec) => {
                st.records += 1;
                st.valid += rec.valid() as u64;
                let p = (mix(rec.eid) >> shift) as usize;
                offs[p + 1] += 1;
                tmp.push(rec);
                parts.push(p as u16);
            }
            None => st.empty_id += 1,
        });
        for i in 0..p1 {
            offs[i + 1] += offs[i];
        }
        let mut cursor: Vec<u32> = offs[..p1].to_vec();
        let n = tmp.len();
        let mut recs: Vec<EvRec> = Vec::with_capacity(n);
        let dst = recs.as_mut_ptr();
        for (rec, &p) in tmp.iter().zip(parts.iter()) {
            let at = cursor[p as usize] as usize;
            cursor[p as usize] += 1;
            // SAFETY: the cursors of all partitions together cover 0..n exactly once.
            unsafe { dst.add(at).write(*rec) };
        }
        // SAFETY: every slot in 0..n was written above.
        unsafe { recs.set_len(n) };
        (EvChunk { recs, offs }, st)
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
