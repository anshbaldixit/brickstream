//! Scattering 16-byte records to counting-sorted positions through software write-combining.
//!
//! A plain scatter stores each record straight into its partition's next slot. With thousands of
//! partitions those slots sit on thousands of different cache lines, so nearly every store misses
//! L1, and each new line is first read from memory (read-for-ownership) only to be overwritten.
//! Here each partition stages its records in one cache line of its own; when the line is full and
//! its destination line belongs to that partition alone, it is written with streaming stores
//! (no read). The few lines at the edges of partitions get ordinary stores.

use std::ops::{Deref, DerefMut};

/// Four 16-byte records: one cache line.
#[repr(C, align(64))]
#[derive(Clone, Copy, Default)]
pub struct Line<T: Copy>([T; 4]);

/// `len` 16-byte records, aligned so that records `4i..4i+4` fill line `i`.
pub struct Lines<T: Copy> {
    lines: Vec<Line<T>>,
    len: usize,
}

impl<T: Copy + Default> Lines<T> {
    /// Room for `len` records, to be written through `as_mut_ptr` and then declared with `written`.
    /// It reads as empty until then.
    pub fn new(len: usize) -> Lines<T> {
        const { assert!(std::mem::size_of::<T>() == 16 && std::mem::align_of::<T>() <= 16) };
        let n = len.div_ceil(4);
        let mut lines: Vec<Line<T>> = Vec::with_capacity(n);
        if n > 0 {
            // SAFETY: line n - 1 is allocated. Its slots past `len` are never written otherwise.
            unsafe { lines.as_mut_ptr().add(n - 1).write(Line::default()) };
        }
        Lines { lines, len }
    }

    /// Declares every record below `len` written.
    ///
    /// SAFETY: every record slot below `len` has been written through `as_mut_ptr`.
    pub unsafe fn written(&mut self) {
        // SAFETY: the caller's promise, and the capacity is exactly these lines.
        unsafe { self.lines.set_len(self.len.div_ceil(4)) };
    }
}

impl<T: Copy> Lines<T> {
    pub fn as_mut_ptr(&mut self) -> *mut Line<T> {
        self.lines.as_mut_ptr()
    }

    fn records(&self) -> usize {
        // Before `written`, no line counts as initialized.
        self.len.min(4 * self.lines.len())
    }
}

impl<T: Copy> Deref for Lines<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        // SAFETY: a line is 4 records with no padding (size 64 = 4 x 16), and the first `records()`
        // records lie in initialized lines.
        unsafe { std::slice::from_raw_parts(self.lines.as_ptr() as *const T, self.records()) }
    }
}

impl<T: Copy> DerefMut for Lines<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        // SAFETY: as for `deref`.
        unsafe { std::slice::from_raw_parts_mut(self.lines.as_mut_ptr() as *mut T, self.records()) }
    }
}

/// One thread's write-combining state: a staged line, a first slot and a cursor per partition.
pub struct Wc<T: Copy> {
    stage: Vec<Line<T>>,
    start: Vec<u32>,
    cur: Vec<u32>,
}

impl<T: Copy + Default> Wc<T> {
    pub const fn new() -> Wc<T> {
        Wc { stage: Vec::new(), start: Vec::new(), cur: Vec::new() }
    }

    /// Starts a scatter into partitions whose first slots are `offs[..offs.len() - 1]` (counting-sort
    /// offsets; `offs` holds one more entry, the total).
    pub fn reset(&mut self, offs: &[u32]) {
        let p = offs.len() - 1;
        self.stage.resize(p, Line::default());
        self.start.clear();
        self.start.extend_from_slice(&offs[..p]);
        self.cur.clear();
        self.cur.extend_from_slice(&offs[..p]);
    }

    /// Appends `rec` to partition `part`; returns its slot.
    ///
    /// SAFETY: `part` is below the partition count, and `dst` has room for every slot the offsets
    /// give (`put` is called at most once per slot).
    #[inline(always)]
    pub unsafe fn put(&mut self, dst: *mut Line<T>, part: usize, rec: T) -> u32 {
        // SAFETY: the caller's promises: `part` indexes all three vectors (each has one entry per
        // partition), and slot `c` (so line `c / 4`) lies inside `dst`.
        unsafe {
            let c = *self.cur.get_unchecked(part);
            *self.cur.get_unchecked_mut(part) = c + 1;
            let slot = (c & 3) as usize;
            let st = self.stage.get_unchecked_mut(part);
            st.0[slot] = rec;
            if slot == 3 {
                let line = dst.add((c >> 2) as usize);
                let start = *self.start.get_unchecked(part);
                if c - 3 >= start {
                    stream(line, st);
                } else {
                    // The partition's first line, shared with the one before: only its own slots.
                    for i in (start & 3) as usize..4 {
                        (*line).0[i] = st.0[i];
                    }
                }
            }
            c
        }
    }

    /// Writes every partition's last, partly filled line, and orders the streaming stores before
    /// any later store (so other threads see the data once they synchronize with this one).
    ///
    /// SAFETY: `dst` is the destination every `put` since `reset` wrote to.
    pub unsafe fn finish(&mut self, dst: *mut Line<T>) {
        for part in 0..self.cur.len() {
            let (s, c) = (self.start[part], self.cur[part]);
            if c == s || c & 3 == 0 {
                continue;
            }
            // SAFETY: slot c - 1 was put, so its line lies inside `dst`.
            let line = unsafe { dst.add((c >> 2) as usize) };
            for i in (c & !3).max(s)..c {
                // SAFETY: as above.
                unsafe { (*line).0[(i & 3) as usize] = self.stage[part].0[(i & 3) as usize] };
            }
        }
        #[cfg(target_arch = "x86_64")]
        // SAFETY: a store fence has no preconditions.
        unsafe {
            core::arch::x86_64::_mm_sfence()
        };
    }
}

/// Copies a staged line to its destination line, bypassing the caches where the CPU allows.
#[inline(always)]
unsafe fn stream<T: Copy>(dst: *mut Line<T>, src: &Line<T>) {
    // SAFETY: the caller passes a valid destination line; both lines are 64-byte aligned, as the
    // aligned 32-byte loads and streaming stores require.
    #[cfg(all(target_arch = "x86_64", target_feature = "avx"))]
    unsafe {
        use core::arch::x86_64::*;
        let (s, d) = (src as *const Line<T> as *const __m256i, dst as *mut __m256i);
        _mm256_stream_si256(d, _mm256_load_si256(s));
        _mm256_stream_si256(d.add(1), _mm256_load_si256(s.add(1)));
    }
    #[cfg(not(all(target_arch = "x86_64", target_feature = "avx")))]
    unsafe {
        *dst = *src;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every record lands in its counting-sorted slot, for partitions that start and end anywhere
    /// within a line, including empty ones and ones inside a single line.
    #[test]
    fn scatter_matches_counting_sort() {
        let mut seed = 0x1234_5678_9ABC_DEF0u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut wc: Wc<[u64; 2]> = Wc::new();
        for round in 0..300 {
            let parts = 1 + (next() % 40) as usize;
            let n = (next() % 500) as usize + round % 3;
            let recs: Vec<([u64; 2], usize)> = (0..n).map(|i| ([i as u64, next()], (next() % parts as u64) as usize)).collect();
            let mut offs = vec![0u32; parts + 1];
            for &(_, p) in &recs {
                offs[p + 1] += 1;
            }
            for i in 1..=parts {
                offs[i] += offs[i - 1];
            }
            let mut want = vec![[0u64; 2]; n];
            let mut cur = offs.clone();
            for &(r, p) in &recs {
                want[cur[p] as usize] = r;
                cur[p] += 1;
            }
            let mut out: Lines<[u64; 2]> = Lines::new(n);
            wc.reset(&offs);
            let mut slots = Vec::new();
            for &(r, p) in &recs {
                slots.push(unsafe { wc.put(out.as_mut_ptr(), p, r) });
            }
            unsafe {
                wc.finish(out.as_mut_ptr());
                out.written();
            }
            assert_eq!(&out[..], &want[..], "round {round}");
            for (i, &s) in slots.iter().enumerate() {
                assert_eq!(out[s as usize], recs[i].0);
            }
        }
    }
}
