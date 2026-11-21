//! Linked IDs (rules.md "Linked IDs").
//!
//! Every link ID goes into one open-addressing table (inserted in parallel with CAS). Each line
//! becomes an edge between two slots; union-find over the slots always keeps the component's
//! smallest ID (byte order, `cmp_ids`) as the root, so the root's key *is* the person's name.
//! The same table then answers user -> person for events and clicks: a key that isn't in it is an
//! unlinked ID, its own person.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
use std::time::Instant;

use rayon::prelude::*;

use crate::events::ParseStats;
use crate::input::{Chunk, Input};
use crate::key::{cmp_ids, key_of, mix};
use crate::scan::for_each_line;

#[repr(C, align(16))]
struct Slot {
    key: AtomicU64,
    name: AtomicU64,
}

pub struct LinkTable {
    slots: Vec<Slot>,
    mask: usize,
    pub ids: usize,
    pub persons: usize,
}

impl LinkTable {
    fn with_capacity(entries: usize) -> LinkTable {
        let cap = (entries + entries / 2).next_power_of_two().max(64);
        let slots = (0..cap).into_par_iter().map(|_| Slot { key: AtomicU64::new(0), name: AtomicU64::new(0) }).collect();
        LinkTable { slots, mask: cap - 1, ids: 0, persons: 0 }
    }

    /// The slot of `k`, inserting it if new. Safe to call from many threads at once.
    fn insert(&self, k: u64) -> u32 {
        let mut i = mix(k) as usize & self.mask;
        loop {
            let cur = self.slots[i].key.load(Relaxed);
            if cur == k {
                return i as u32;
            }
            if cur == 0 {
                match self.slots[i].key.compare_exchange(0, k, Relaxed, Relaxed) {
                    Ok(_) => return i as u32,
                    Err(actual) if actual == k => return i as u32,
                    Err(_) => {}
                }
            }
            i = (i + 1) & self.mask;
        }
    }

    /// The person (name key) of a user key.
    #[inline(always)]
    pub fn person(&self, k: u64) -> u64 {
        let mut i = mix(k) as usize & self.mask;
        loop {
            let s = &self.slots[i];
            let cur = s.key.load(Relaxed);
            if cur == k {
                return s.name.load(Relaxed);
            }
            if cur == 0 {
                return k;
            }
            i = (i + 1) & self.mask;
        }
    }

    /// Prefetches the home slot of `k` (call a few iterations before `person(k)`).
    #[inline(always)]
    pub fn prefetch(&self, k: u64) {
        let i = mix(k) as usize & self.mask;
        #[cfg(target_arch = "x86_64")]
        // SAFETY: prefetch is a hint; the address is inside `slots`.
        unsafe {
            core::arch::x86_64::_mm_prefetch(self.slots.as_ptr().add(i) as *const i8, core::arch::x86_64::_MM_HINT_T0);
        }
    }

    /// Replaces every key in `keys` by its person, prefetching ahead.
    #[inline]
    pub fn resolve(&self, keys: &mut [u64]) {
        const AHEAD: usize = 16;
        for k in keys.iter().take(AHEAD) {
            self.prefetch(*k);
        }
        for i in 0..keys.len() {
            if i + AHEAD < keys.len() {
                self.prefetch(keys[i + AHEAD]);
            }
            keys[i] = self.person(keys[i]);
        }
    }
}

/// The root of `x`, halving the path on the way (safe concurrently: a non-root's parent only ever
/// moves to another of its ancestors, because trees only merge).
#[inline(always)]
fn find(parent: &[AtomicU32], mut x: u32) -> u32 {
    loop {
        let p = parent[x as usize].load(Relaxed);
        if p == x {
            return x;
        }
        let g = parent[p as usize].load(Relaxed);
        if g != p {
            parent[x as usize].store(g, Relaxed);
        }
        x = g;
    }
}

#[inline(always)]
fn root(parent: &[AtomicU32], mut x: u32) -> u32 {
    loop {
        let p = parent[x as usize].load(Relaxed);
        if p == x {
            return x;
        }
        x = p;
    }
}

pub struct LinkStats {
    pub parse: ParseStats,
    pub edges: usize,
    pub ms_parse: f64,
    pub ms_insert: f64,
    pub ms_union: f64,
    pub ms_names: f64,
}

pub fn build(files: &[Input], chunks: &[Chunk]) -> (LinkTable, LinkStats) {
    let t = Instant::now();
    // 1. Parse: drop malformed lines and lines with an empty side; skip self-links (no effect).
    let parsed: Vec<(Vec<(u64, u64)>, ParseStats)> = chunks
        .par_iter()
        .map(|c| {
            let data: &[u8] = &files[c.file as usize].data;
            let mut pairs = Vec::with_capacity((c.end - c.start) / 20);
            let mut st = ParseStats::default();
            st.lines = for_each_line::<2, _>(data, c.start, c.end, |f| {
                if f[0].is_empty() || f[1].is_empty() {
                    st.empty_id += 1;
                    return;
                }
                st.records += 1;
                let (a, b) = (key_of(f[0]), key_of(f[1]));
                if a != b {
                    pairs.push((a, b));
                }
            });
            (pairs, st)
        })
        .collect();
    let mut stats = LinkStats {
        parse: ParseStats::default(),
        edges: 0,
        ms_parse: 0.0,
        ms_insert: 0.0,
        ms_union: 0.0,
        ms_names: 0.0,
    };
    for (pairs, st) in &parsed {
        stats.parse.add(*st);
        stats.edges += pairs.len();
    }
    stats.ms_parse = ms(t);

    // 2. Insert both sides of every edge.
    let t = Instant::now();
    let mut table = LinkTable::with_capacity(2 * stats.edges);
    let edges: Vec<Vec<(u32, u32)>> = parsed
        .par_iter()
        .map(|(pairs, _)| pairs.iter().map(|&(a, b)| (table.insert(a), table.insert(b))).collect())
        .collect();
    drop(parsed);
    stats.ms_insert = ms(t);

    // 3. Union-find over slots, in parallel and lock-free: a root is only ever linked (by CAS on its
    // own parent) under a root with a smaller key, so keys decrease along every path and the root of
    // each component is its smallest key, the person's name.
    let t = Instant::now();
    let cap = table.slots.len();
    let parent: Vec<AtomicU32> = (0..cap as u32).into_par_iter().map(AtomicU32::new).collect();
    let key = |i: u32| table.slots[i as usize].key.load(Relaxed);
    edges.par_iter().for_each(|list| {
        for &(a, b) in list {
            let (mut a, mut b) = (a, b);
            loop {
                a = find(&parent, a);
                b = find(&parent, b);
                if a == b {
                    break;
                }
                let (hi, lo) = if cmp_ids(key(a), key(b)).is_lt() { (b, a) } else { (a, b) };
                if parent[hi as usize].compare_exchange(hi, lo, Relaxed, Relaxed).is_ok() {
                    break;
                }
            }
        }
    });
    drop(edges);
    stats.ms_union = ms(t);

    // 4. Every occupied slot gets its person's name: the root's key.
    let t = Instant::now();
    let (ids, persons) = table
        .slots
        .par_iter()
        .enumerate()
        .map(|(i, s)| {
            let k = s.key.load(Relaxed);
            if k == 0 {
                return (0usize, 0usize);
            }
            let r = root(&parent, i as u32);
            s.name.store(key(r), Relaxed);
            (1, (r == i as u32) as usize)
        })
        .reduce(|| (0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    table.ids = ids;
    table.persons = persons;
    stats.ms_names = ms(t);
    (table, stats)
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table_from(pairs: &[(&[u8], &[u8])]) -> LinkTable {
        let mut text = b"user_id_a,user_id_b\n".to_vec();
        for (a, b) in pairs {
            text.extend_from_slice(a);
            text.push(b',');
            text.extend_from_slice(b);
            text.push(b'\n');
        }
        let input = Input { path: "t".into(), data: crate::input::Data::Owned(text) };
        let files = vec![input];
        let chunks = crate::input::chunks(&files, 0, 7);
        build(&files, &chunks).0
    }

    #[test]
    fn chains_names_and_mixed_keys() {
        let t = table_from(&[
            (b"u3", b"u9"),
            (b"u9", b"u2"),
            (b"U7", b"u9"),
            (b"u10", b"u10"),
            (b"zz_this_id_is_long_1", b"u5"),
            (b"u5", b"u6"),
            (b"a_this_id_is_long_2", b"u6"),
        ]);
        let p = |s: &[u8]| t.person(key_of(s));
        for id in [&b"u3"[..], b"u9", b"u2", b"U7"] {
            assert_eq!(p(id), key_of(b"U7"));
        }
        assert_eq!(p(b"u10"), key_of(b"u10"));
        assert_eq!(p(b"u1"), key_of(b"u1"));
        // A long (interned) ID that is the byte-wise smallest names the person...
        for id in [&b"u5"[..], b"u6", b"zz_this_id_is_long_1", b"a_this_id_is_long_2"] {
            assert_eq!(p(id), key_of(b"a_this_id_is_long_2"));
        }
    }

    #[test]
    fn long_chain_any_order() {
        // A 300-hop chain listed in a scrambled order.
        let ids: Vec<Vec<u8>> = (0..301).map(|i| format!("c{:04}", (i * 7919) % 10007).into_bytes()).collect();
        let mut pairs: Vec<(&[u8], &[u8])> = ids.windows(2).map(|w| (&w[0][..], &w[1][..])).collect();
        let len = pairs.len();
        for i in 0..len {
            pairs.swap(i, (i * 131) % len);
        }
        let t = table_from(&pairs);
        let smallest = ids.iter().min().unwrap();
        for id in &ids {
            assert_eq!(t.person(key_of(id)), key_of(smallest));
        }
    }
}
