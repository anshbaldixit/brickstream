//! Clicks (rules.md "Clicks", steps 1-3).
//!
//! Malformed lines and lines with an empty `click_id` are dropped first. Dedup on `click_id` is
//! skipped on purpose: the rules promise copies are identical, the credited click is the maximum of
//! (ts, then smallest click_id) over a set and assists are the set of campaigns, so an identical
//! copy can't change either. Invalid clicks (empty user, bad or out-of-window ts, unknown campaign)
//! are dropped here, before any choice is made, so they can never hide a valid click.

use crate::key::key_of;
use crate::lookups::{Campaigns, Window};
use crate::scan::for_each_line;
use crate::ts::parse_ts;
use crate::events::ParseStats;

/// One valid click (24 bytes). `who` is the user key in phase 1 and the person key after phase 2.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct ClickRec {
    pub who: u64,
    pub cid: u64,
    /// `ts - window.base` in ms.
    pub ts: u32,
    /// Index into campaigns.csv.
    pub camp: u32,
}

pub struct ClickParser<'a> {
    pub window: Window,
    pub campaigns: &'a Campaigns,
}

impl ClickParser<'_> {
    /// `Ok(Some)` for a valid click, `Ok(None)` for an invalid one, `Err` for an empty click_id.
    #[inline(always)]
    pub fn parse(&self, f: &[&[u8]; 4]) -> Result<Option<ClickRec>, ()> {
        if f[0].is_empty() {
            return Err(());
        }
        if f[1].is_empty() || f[2].is_empty() {
            return Ok(None);
        }
        let w = &self.window;
        let ts = match parse_ts(f[3]) {
            Some(t) if t >= w.base && t < w.end => t,
            _ => return Ok(None),
        };
        let Some(camp) = self.campaigns.index(key_of(f[2])) else {
            return Ok(None);
        };
        Ok(Some(ClickRec { who: key_of(f[1]), cid: key_of(f[0]), ts: (ts - w.base) as u32, camp }))
    }
}

pub fn parse_chunk(parser: &ClickParser, data: &[u8], start: usize, end: usize, out: &mut Vec<ClickRec>) -> ParseStats {
    let mut st = ParseStats::default();
    st.lines = for_each_line::<4, _>(data, start, end, |f| match parser.parse(f) {
        Ok(Some(c)) => {
            st.records += 1;
            st.valid += 1;
            out.push(c);
        }
        Ok(None) => st.records += 1,
        Err(()) => st.empty_id += 1,
    });
    st
}
