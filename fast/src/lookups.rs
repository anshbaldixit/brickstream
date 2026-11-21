//! The small inputs: manifest.json (the window), products.csv (category), campaigns.csv (channel).

use std::path::Path;

use crate::key::key_of;
use crate::scan::for_each_line;
use crate::table::KeyMap;
use crate::ts::{parse_ts, MS_PER_DAY};

pub const SEVEN_DAYS_MS: i64 = 7 * MS_PER_DAY;

/// The event window `[start, end)` in epoch ms, and the base all `u32` time offsets count from:
/// `start - 7 days`, the earliest instant a valid click can have.
#[derive(Clone, Copy, Debug)]
pub struct Window {
    pub start: i64,
    pub end: i64,
    pub base: i64,
}

impl Window {
    /// First and last UTC day (days since the epoch) a session can start on.
    pub fn first_day(&self) -> i64 {
        self.start.div_euclid(MS_PER_DAY)
    }
    pub fn last_day(&self) -> i64 {
        (self.end - 1).div_euclid(MS_PER_DAY)
    }
    pub fn days(&self) -> usize {
        (self.last_day() - self.first_day() + 1) as usize
    }
}

fn die(msg: String) -> ! {
    eprintln!("fast: {msg}");
    std::process::exit(2);
}

pub fn read_manifest(path: &Path) -> Window {
    let text = std::fs::read(path).unwrap_or_else(|e| die(format!("cannot read {}: {e}", path.display())));
    let json: serde_json::Value =
        serde_json::from_slice(&text).unwrap_or_else(|e| die(format!("bad manifest {}: {e}", path.display())));
    let get = |name: &str| -> i64 {
        let s = json.get(name).and_then(|v| v.as_str()).unwrap_or_else(|| die(format!("manifest has no {name}")));
        parse_ts(s.as_bytes()).unwrap_or_else(|| die(format!("manifest {name} is not a timestamp: {s:?}")))
    };
    let (start, end) = (get("window_start"), get("window_end"));
    if end <= start {
        die(format!("empty window: {start}..{end}"));
    }
    let window = Window { start, end, base: start - SEVEN_DAYS_MS };
    // Every in-window instant is stored as a u32 offset from `base`.
    if (end - window.base) as u128 > u32::MAX as u128 {
        die(format!("window too long for u32 offsets: {start}..{end}"));
    }
    window
}

fn read_small(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| die(format!("cannot read {}: {e}", path.display())))
}

/// For each line after the header: the two unquoted fields (malformed lines skipped).
fn two_columns<'a>(data: &'a [u8], mut f: impl FnMut(&'a [u8], &'a [u8])) {
    let start = match memchr::memchr(b'\n', data) {
        Some(h) => h + 1,
        None => return,
    };
    for_each_line::<2, _>(data, start, data.len(), |fields| f(fields[0], fields[1]));
}

pub struct Products {
    map: KeyMap,
    /// Category names in byte order; a category's index is its rank, so the smallest index wins a tie.
    pub categories: Vec<Vec<u8>>,
    /// The rank of `unknown`, the category of a product missing from products.csv.
    pub unknown: u32,
}

impl Products {
    pub fn read(path: &Path) -> Products {
        let data = read_small(path);
        let mut rows: Vec<(&[u8], &[u8])> = Vec::new();
        two_columns(&data, |id, cat| {
            if !id.is_empty() {
                rows.push((id, cat));
            }
        });
        let mut categories: Vec<Vec<u8>> = rows.iter().map(|r| r.1.to_vec()).collect();
        categories.push(b"unknown".to_vec());
        categories.sort_unstable();
        categories.dedup();
        if categories.len() >= 1 << 24 {
            die("more than 2^24 categories".into());
        }
        let rank = |c: &[u8]| categories.binary_search_by(|x| x.as_slice().cmp(c)).unwrap() as u32;
        let entries: Vec<(u64, u32)> = rows.iter().map(|&(id, cat)| (key_of(id), rank(cat))).collect();
        let unknown = rank(b"unknown");
        Products { map: KeyMap::new(&entries), categories, unknown }
    }

    #[inline(always)]
    pub fn category(&self, product: u64) -> u32 {
        self.map.get(product).unwrap_or(self.unknown)
    }
}

pub struct Campaigns {
    map: KeyMap,
    pub ids: Vec<Vec<u8>>,
    pub channels: Vec<Vec<u8>>,
}

impl Campaigns {
    pub fn read(path: &Path) -> Campaigns {
        let data = read_small(path);
        let mut ids = Vec::new();
        let mut channels = Vec::new();
        let mut entries = Vec::new();
        two_columns(&data, |id, channel| {
            if !id.is_empty() {
                entries.push((key_of(id), ids.len() as u32));
                ids.push(id.to_vec());
                channels.push(channel.to_vec());
            }
        });
        Campaigns { map: KeyMap::new(&entries), ids, channels }
    }

    /// The index of a campaign in campaigns.csv; `organic` is `len()`.
    #[inline(always)]
    pub fn index(&self, campaign: u64) -> Option<u32> {
        self.map.get(campaign)
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }
}
