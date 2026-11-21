//! Phase 3: sessions, credit and assists, per person (rules.md Events steps 5-7, Clicks steps 4-7).
//!
//! A P2 partition holds every surviving event and valid click of its persons. Events and clicks
//! are grouped by person (local hash map + counting scatter), each person's events are sorted by ts
//! and their clicks by (ts ascending, click_id descending). A new session starts when the gap from
//! the previous event is over 30 minutes (equal timestamps share a session). Two pointers keep the
//! person's clicks with `start - 7 d <= ts <= start`: the last one is the credit (latest ts, then
//! smallest click_id), and a counted set of their campaigns gives the assists.

use std::cell::RefCell;
use std::time::Instant;

use crate::clicks::ClickRec;
use crate::dedup::{Out, PRec, SIDE_BITS};
use crate::key::{cmp_ids, mix};
use crate::lookups::{Window, SEVEN_DAYS_MS};
use crate::ts::MS_PER_DAY;

const GAP_MS: u32 = 1_800_000;
const SEVEN_DAYS: u32 = SEVEN_DAYS_MS as u32;
pub const NO_CATEGORY: u32 = u32::MAX;

/// One user_daily row.
#[derive(Clone, Copy, Debug)]
pub struct Row {
    pub person: u64,
    pub day: i32,
    pub sessions: u32,
    pub events: u32,
    pub revenue: u64,
    /// Category rank, or NO_CATEGORY (null) without purchases.
    pub top: u32,
}

/// campaign_daily counters for one (campaign, day).
#[derive(Clone, Copy, Default, Debug)]
pub struct Acc {
    pub sessions: u64,
    pub purchasing: u64,
    pub revenue: u64,
    pub assisted: u64,
}

/// A person whose work is worth logging.
#[derive(Clone, Copy, Debug)]
pub struct Big {
    pub person: u64,
    pub events: usize,
    pub clicks: usize,
    pub sessions: u64,
    pub micros: u64,
}

#[derive(Clone, Copy)]
struct Cl {
    ts: u32,
    camp: u32,
    cid: u64,
}

#[derive(Default)]
struct Scratch {
    keys: Vec<u64>,
    slot: Vec<u32>,
    persons: Vec<u64>,
    counts: Vec<u32>,
    ccounts: Vec<u32>,
    of_event: Vec<u32>,
    of_click: Vec<u32>,
    evs: Vec<u64>,
    cls: Vec<Cl>,
    cnt: Vec<u32>,
    pos: Vec<u32>,
    present: Vec<u32>,
    cat_sum: Vec<u64>,
    touched: Vec<u32>,
}

thread_local! {
    static SCRATCH: RefCell<Scratch> = RefCell::new(Scratch::default());
}

pub struct Ctx<'a> {
    pub window: Window,
    pub outs: &'a [Out],
    pub campaigns: usize,
    pub categories: usize,
    pub days: usize,
}

impl Ctx<'_> {
    #[inline(always)]
    fn purchase(&self, pay: u32) -> (u32, u32) {
        let v = pay - 1;
        self.outs[(v >> SIDE_BITS) as usize].purchases[(v & ((1 << SIDE_BITS) - 1)) as usize]
    }
}

/// Sessionizes P2 partition `q`. Appends user_daily rows to `rows` and campaign counters to `acc`
/// (indexed `campaign * days + day_index`, organic = `campaigns`).
pub fn partition(ctx: &Ctx, q: usize, rows: &mut Vec<Row>, acc: &mut [Acc], big: &mut Vec<Big>) -> u64 {
    let ev_slices: Vec<&[PRec]> = ctx.outs.iter().map(|o| &o.events[q][..]).collect();
    let cl_slices: Vec<&[ClickRec]> = ctx.outs.iter().map(|o| &o.clicks[q][..]).collect();
    let n: usize = ev_slices.iter().map(|s| s.len()).sum();
    if n == 0 {
        return 0;
    }
    let nc: usize = cl_slices.iter().map(|s| s.len()).sum();
    SCRATCH.with(|cell| {
        let mut guard = cell.borrow_mut();
        let sc = &mut *guard;
        // 1. Local person index for every event, and event counts per person.
        let cap = (2 * n).next_power_of_two().max(64);
        let mask = cap - 1;
        sc.keys.clear();
        sc.keys.resize(cap, 0);
        sc.slot.resize(cap, 0);
        sc.persons.clear();
        sc.counts.clear();
        sc.of_event.clear();
        sc.of_event.reserve(n);
        for s in &ev_slices {
            for r in s.iter() {
                let mut h = mix(r.person) as usize & mask;
                let idx = loop {
                    let k = sc.keys[h];
                    if k == r.person {
                        break sc.slot[h];
                    }
                    if k == 0 {
                        let idx = sc.persons.len() as u32;
                        sc.keys[h] = r.person;
                        sc.slot[h] = idx;
                        sc.persons.push(r.person);
                        sc.counts.push(0);
                        break idx;
                    }
                    h = (h + 1) & mask;
                };
                sc.counts[idx as usize] += 1;
                sc.of_event.push(idx);
            }
        }
        let m = sc.persons.len();
        // 2. Clicks of persons who have events (others can't matter).
        sc.ccounts.clear();
        sc.ccounts.resize(m, 0);
        sc.of_click.clear();
        sc.of_click.reserve(nc);
        for s in &cl_slices {
            for c in s.iter() {
                let mut h = mix(c.who) as usize & mask;
                let idx = loop {
                    let k = sc.keys[h];
                    if k == c.who {
                        break sc.slot[h];
                    }
                    if k == 0 {
                        break u32::MAX;
                    }
                    h = (h + 1) & mask;
                };
                if idx != u32::MAX {
                    sc.ccounts[idx as usize] += 1;
                }
                sc.of_click.push(idx);
            }
        }
        // 3. Counting scatter: events as (ts << 32 | pay), clicks as Cl, grouped by person.
        let mut off = 0u32;
        for c in sc.counts.iter_mut() {
            let len = *c;
            *c = off;
            off += len;
        }
        sc.evs.clear();
        sc.evs.resize(n, 0);
        let mut e = 0;
        for s in &ev_slices {
            for r in s.iter() {
                let idx = sc.of_event[e] as usize;
                e += 1;
                let at = sc.counts[idx] as usize;
                sc.counts[idx] += 1;
                sc.evs[at] = ((r.ts as u64) << 32) | r.pay as u64;
            }
        }
        let mut coff = 0u32;
        for c in sc.ccounts.iter_mut() {
            let len = *c;
            *c = coff;
            coff += len;
        }
        sc.cls.clear();
        sc.cls.resize(coff as usize, Cl { ts: 0, camp: 0, cid: 0 });
        let mut e = 0;
        for s in &cl_slices {
            for c in s.iter() {
                let idx = sc.of_click[e];
                e += 1;
                if idx != u32::MAX {
                    let at = sc.ccounts[idx as usize] as usize;
                    sc.ccounts[idx as usize] += 1;
                    sc.cls[at] = Cl { ts: c.ts, camp: c.camp, cid: c.cid };
                }
            }
        }
        // counts[i] / ccounts[i] are now the END of person i's range; starts are the previous end.
        sc.cnt.resize(ctx.campaigns, 0);
        sc.pos.resize(ctx.campaigns, 0);
        sc.cat_sum.resize(ctx.categories, 0);
        let mut sessions_total = 0u64;
        let (mut ev_start, mut cl_start) = (0usize, 0usize);
        for i in 0..m {
            let (ev_end, cl_end) = (sc.counts[i] as usize, sc.ccounts[i] as usize);
            let t = (ev_end - ev_start > 20_000).then(Instant::now);
            let who = sc.persons[i];
            let s = person(ctx, sc, who, ev_start, ev_end, cl_start, cl_end, rows, acc);
            sessions_total += s;
            if let Some(t) = t {
                big.push(Big {
                    person: who,
                    events: ev_end - ev_start,
                    clicks: cl_end - cl_start,
                    sessions: s,
                    micros: t.elapsed().as_micros() as u64,
                });
            }
            ev_start = ev_end;
            cl_start = cl_end;
        }
        sessions_total
    })
}

/// One person: sort, sessionize, credit, assist, and emit their user_daily rows.
#[allow(clippy::too_many_arguments)]
fn person(
    ctx: &Ctx,
    sc: &mut Scratch,
    who: u64,
    e0: usize,
    e1: usize,
    c0: usize,
    c1: usize,
    rows: &mut Vec<Row>,
    acc: &mut [Acc],
) -> u64 {
    let evs = &mut sc.evs[e0..e1];
    evs.sort_unstable();
    let cls = &mut sc.cls[c0..c1];
    if cls.len() > 1 {
        cls.sort_unstable_by(|a, b| a.ts.cmp(&b.ts).then_with(|| cmp_ids(b.cid, a.cid)));
    }
    let organic = ctx.campaigns;
    let days = ctx.days;
    let base = ctx.window.base;
    let first_day = ctx.window.first_day();
    let (mut lo, mut hi) = (0usize, 0usize);
    let mut sessions = 0u64;
    let mut day_row: Option<Row> = None;
    let mut i = 0;
    while i < evs.len() {
        let start = (evs[i] >> 32) as u32;
        let day = (base + start as i64).div_euclid(MS_PER_DAY);
        let d = (day - first_day) as usize;
        if day_row.is_some_and(|r| r.day as i64 != day) {
            emit(day_row.take().unwrap(), &mut sc.cat_sum, &mut sc.touched, rows);
        }
        let row = day_row.get_or_insert(Row { person: who, day: day as i32, sessions: 0, events: 0, revenue: 0, top: NO_CATEGORY });
        // The session: events until a gap of more than 30 minutes.
        let mut prev = start;
        let mut revenue = 0u64;
        let mut purchased = false;
        let mut j = i;
        while j < evs.len() {
            let ts = (evs[j] >> 32) as u32;
            if ts - prev > GAP_MS {
                break;
            }
            prev = ts;
            let pay = evs[j] as u32;
            if pay != 0 {
                let (price, cat) = ctx.purchase(pay);
                revenue += price as u64;
                purchased = true;
                if sc.cat_sum[cat as usize] == 0 {
                    sc.touched.push(cat);
                }
                sc.cat_sum[cat as usize] += price as u64;
            }
            j += 1;
        }
        row.sessions += 1;
        row.events += (j - i) as u32;
        row.revenue += revenue;
        // Credit and assists: clicks with start - 7 d <= ts <= start.
        while hi < cls.len() && cls[hi].ts <= start {
            let c = cls[hi].camp as usize;
            if sc.cnt[c] == 0 {
                sc.pos[c] = sc.present.len() as u32;
                sc.present.push(c as u32);
            }
            sc.cnt[c] += 1;
            hi += 1;
        }
        let low = start - SEVEN_DAYS; // start >= window start = base + 7 d
        while lo < hi && cls[lo].ts < low {
            let c = cls[lo].camp as usize;
            sc.cnt[c] -= 1;
            if sc.cnt[c] == 0 {
                let p = sc.pos[c] as usize;
                let last = sc.present.pop().unwrap();
                if last as usize != c {
                    sc.present[p] = last;
                    sc.pos[last as usize] = p as u32;
                }
            }
            lo += 1;
        }
        let credited = if hi > lo { cls[hi - 1].camp as usize } else { organic };
        let a = &mut acc[credited * days + d];
        a.sessions += 1;
        a.purchasing += purchased as u64;
        a.revenue += revenue;
        for &c in &sc.present {
            acc[c as usize * days + d].assisted += 1;
        }
        sessions += 1;
        i = j;
    }
    if let Some(r) = day_row {
        emit(r, &mut sc.cat_sum, &mut sc.touched, rows);
    }
    for &c in &sc.present {
        sc.cnt[c as usize] = 0;
    }
    sc.present.clear();
    sessions
}

/// Finishes a person-day: top category = highest summed price, ties to the smallest rank.
#[inline]
fn emit(mut row: Row, cat_sum: &mut [u64], touched: &mut Vec<u32>, rows: &mut Vec<Row>) {
    let mut best = (0u64, NO_CATEGORY);
    for &c in touched.iter() {
        let s = cat_sum[c as usize];
        if s > best.0 || (s == best.0 && c < best.1) {
            best = (s, c);
        }
        cat_sum[c as usize] = 0;
    }
    touched.clear();
    row.top = best.1;
    rows.push(row);
}
