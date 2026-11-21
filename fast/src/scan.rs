//! Lines and fields under the rules' CSV dialect (rules.md "CSV dialect").
//!
//! Only `\n` ends a line; `\r` is ordinary data. A line has exactly N fields separated by commas
//! (commas always separate, even inside quotes). A field may be wrapped in double quotes, which are
//! syntax: `"u1"` is `u1` and `""` is empty (null). A quote anywhere else, a lone `"`, `"""`, `"a""b"`,
//! the wrong number of fields, or an empty line make the line malformed, and it is dropped.
//!
//! Fast path (AVX2): for a line starting at `pos` with 128 readable bytes, build bitmasks of `\n`,
//! `,` and `"` over the window. The line is well-formed iff it has exactly N-1 commas before its `\n`
//! and its quote mask equals exactly {first byte, last byte} of every field that is at least 2 bytes
//! long and starts and ends with a quote. Lines that don't fit the window (long lines, the last bytes
//! of a file) go through `split_line`, the scalar reference with the same semantics.

use std::cell::RefCell;

#[derive(Default, Clone, Copy, Debug)]
pub struct LineStats {
    /// Lines seen (well-formed or not), excluding the header.
    pub lines: u64,
    pub malformed: u64,
}

impl LineStats {
    pub fn add(&mut self, other: LineStats) {
        self.lines += other.lines;
        self.malformed += other.malformed;
    }
}

/// Strips the quotes of one field, or `None` if its quotes make the line malformed.
#[inline]
fn unquote(f: &[u8]) -> Option<&[u8]> {
    if memchr::memchr(b'"', f).is_none() {
        return Some(f);
    }
    let n = f.len();
    if n >= 2 && f[0] == b'"' && f[n - 1] == b'"' && memchr::memchr(b'"', &f[1..n - 1]).is_none() {
        Some(&f[1..n - 1])
    } else {
        None
    }
}

/// The scalar reference: the N unquoted values of one line (without its `\n`), or `None` if malformed.
pub fn split_line<const N: usize>(line: &[u8]) -> Option<[&[u8]; N]> {
    let mut out: [&[u8]; N] = [&[]; N];
    let mut count = 0;
    for field in line.split(|&c| c == b',') {
        if count == N {
            return None;
        }
        out[count] = unquote(field)?;
        count += 1;
    }
    if count == N { Some(out) } else { None }
}

/// Bitmasks of `\n`, `,` and `"` over 64 bytes.
#[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
#[inline(always)]
fn masks64(block: &[u8; 64]) -> (u64, u64, u64) {
    use core::arch::x86_64::*;
    // SAFETY: AVX2 is enabled at compile time and `block` has 64 readable bytes.
    unsafe {
        let p = block.as_ptr();
        let lo = _mm256_loadu_si256(p as *const __m256i);
        let hi = _mm256_loadu_si256(p.add(32) as *const __m256i);
        let m = |v: __m256i, c: u8| _mm256_movemask_epi8(_mm256_cmpeq_epi8(v, _mm256_set1_epi8(c as i8))) as u32 as u64;
        (
            m(lo, b'\n') | m(hi, b'\n') << 32,
            m(lo, b',') | m(hi, b',') << 32,
            m(lo, b'"') | m(hi, b'"') << 32,
        )
    }
}

#[cfg(not(all(target_arch = "x86_64", target_feature = "avx2")))]
#[inline(always)]
fn masks64(block: &[u8; 64]) -> (u64, u64, u64) {
    let (mut n, mut c, mut q) = (0u64, 0u64, 0u64);
    for (i, &b) in block.iter().enumerate() {
        n |= ((b == b'\n') as u64) << i;
        c |= ((b == b',') as u64) << i;
        q |= ((b == b'"') as u64) << i;
    }
    (n, c, q)
}

const MAX_FIELDS: usize = 16;
/// Bytes per window. Every window ends at a line end, so no line spans two windows.
const WINDOW: usize = 16 * 1024;
/// Room for a window's positions: at most 64 per block, one block past the window (a missing
/// final `\n`), up to 8 unconditional writes past the count, and the sentinels.
const SLOTS: usize = WINDOW + 256;

/// Per-thread buffers for one window's structural index.
struct Scratch {
    /// Offsets of field starts (one past the opening quote of a quoted field), in order.
    starts: Vec<u32>,
    /// Offsets of line ends: each `\n`, and the end of a file's last line if it has none.
    ends: Vec<u32>,
    /// Offsets where quoting breaks the dialect; a line holding one is malformed.
    bads: Vec<u32>,
}

thread_local! {
    static SCRATCH: RefCell<Scratch> =
        RefCell::new(Scratch { starts: vec![0; SLOTS], ends: vec![0; SLOTS], bads: vec![0; SLOTS] });
}

/// Calls `f` with the N unquoted values of every well-formed line in `buf[start..end]`.
/// `start` must be a line start and `end` a line end (just after a `\n`, or the end of the file).
///
/// The bytes are processed in windows of whole lines; lines longer than a window go through
/// `split_line`. Pass A builds a window's structural index from AVX2 bitmasks of `\n`, `,` and `"`
/// per 64 bytes: field starts, line ends and quoting errors, each as a list of offsets. Pass B
/// walks the lines: a line is well-formed iff exactly N field starts precede its end and no quoting
/// error lies in it; its fields then follow from the starts alone.
pub fn for_each_line<'a, const N: usize, F: FnMut(&[&'a [u8]; N])>(
    buf: &'a [u8],
    start: usize,
    end: usize,
    mut f: F,
) -> LineStats {
    const { assert!(N >= 2 && N <= MAX_FIELDS) };
    debug_assert!(end <= buf.len());
    let mut stats = LineStats::default();
    SCRATCH.with(|cell| {
        let mut guard = cell.borrow_mut();
        let sc = &mut *guard;
        let mut ws = start;
        while ws < end {
            let mut we = (ws + WINDOW).min(end);
            if we < end {
                match memchr::memrchr(b'\n', &buf[ws..we]) {
                    Some(i) => we = ws + i + 1,
                    None => {
                        // A line longer than a window goes through the scalar reference.
                        let le = memchr::memchr(b'\n', &buf[ws..end]).map_or(end, |i| ws + i);
                        stats.lines += 1;
                        match split_line::<N>(&buf[ws..le]) {
                            Some(fields) => f(&fields),
                            None => stats.malformed += 1,
                        }
                        ws = le + 1;
                        continue;
                    }
                }
            }
            window::<N, F>(buf, ws, we, sc, &mut f, &mut stats);
            ws = we;
        }
    });
    stats
}

/// Writes `base + i` for each set bit `i` of `m` at `dst[*n..]`, the first `K` unconditionally
/// (entries past the count are overwritten later), and advances `*n` by the count.
///
/// SAFETY: `dst` must have room for `*n + max(K, popcount(m))` entries.
#[inline(always)]
unsafe fn flatten<const K: usize>(dst: *mut u32, n: &mut usize, base: u32, mut m: u64) {
    let cnt = m.count_ones() as usize;
    // SAFETY: the caller's promise covers every write below.
    unsafe {
        let d = dst.add(*n);
        for j in 0..K {
            *d.add(j) = base + m.trailing_zeros();
            m &= m.wrapping_sub(1);
        }
        let mut j = K;
        while j < cnt {
            *d.add(j) = base + m.trailing_zeros();
            m &= m.wrapping_sub(1);
            j += 1;
        }
    }
    *n += cnt;
}

#[inline(always)]
fn window<'a, const N: usize, F: FnMut(&[&'a [u8]; N])>(
    buf: &'a [u8],
    ws: usize,
    we: usize,
    sc: &mut Scratch,
    f: &mut F,
    stats: &mut LineStats,
) {
    let w: &'a [u8] = &buf[ws..we];
    let wlen = w.len();
    // A file's last line may lack its `\n`: a virtual one at `wlen` ends it.
    let open_end = w[wlen - 1] != b'\n';
    let nblocks = (wlen + open_end as usize).div_ceil(64);
    debug_assert!(64 * nblocks + 64 + MAX_FIELDS + 1 <= SLOTS);

    // Pass A. Per block, with `sep` = commas and line ends and `first` = a field's first byte:
    // a quote at a field's first byte opens the field, any other quote must close it. So every
    // other quote must be followed by a separator, an opening quote must not be, and a field's
    // separator must follow a quote exactly when the field opened with one. The last test carries
    // each opening quote to its field's separator with one addition (the carry ripples through the
    // field's non-separator bytes). Together that is the rules' dialect: a field either holds no
    // quote, or is at least 2 bytes that start and end with `"` and hold no other `"`.
    let (mut ns, mut ne, mut nb) = (0usize, 0usize, 0usize);
    let (mut prev_sep, mut prev_open, mut prev_other, mut carry) = (1u64, 0u64, 0u64, 0u64);
    let (ps, pe, pb) = (sc.starts.as_mut_ptr(), sc.ends.as_mut_ptr(), sc.bads.as_mut_ptr());
    let mut pad = [0u8; 64];
    for b in 0..nblocks {
        let p = ws + 64 * b;
        let block: &[u8; 64] = if p + 64 <= buf.len() {
            buf[p..p + 64].try_into().unwrap()
        } else {
            let n = buf.len() - p;
            pad[..n].copy_from_slice(&buf[p..]);
            pad[n..].fill(0);
            &pad
        };
        let (mut nl, mut cm, mut qt) = masks64(block);
        let rem = wlen - 64 * b;
        if rem < 64 {
            let keep = (1u64 << rem) - 1;
            nl &= keep;
            cm &= keep;
            qt &= keep;
            nl |= (open_end as u64) << rem;
        }
        let sep = nl | cm;
        let first = (sep << 1) | prev_sep;
        let open = qt & first;
        let other = qt & !first;
        let after_open = (open << 1) | prev_open;
        let after_other = (other << 1) | prev_other;
        let (s, k1) = (!sep).overflowing_add(open);
        let (opened, k2) = s.overflowing_add(carry);
        let bad = (after_other & !sep) | (after_open & sep) | ((opened ^ (after_other | after_open)) & sep);
        let starts = (first & !qt) | after_open;
        carry = (k1 | k2) as u64;
        prev_sep = sep >> 63;
        prev_open = open >> 63;
        prev_other = other >> 63;
        let base = (64 * b) as u32;
        // SAFETY: each list gets at most 64 entries per block (and at most 8 written past its
        // count), so all writes stay below 64 * nblocks + 64 <= SLOTS.
        unsafe {
            flatten::<8>(ps, &mut ns, base, starts);
            flatten::<2>(pe, &mut ne, base, nl);
            if bad != 0 {
                flatten::<0>(pb, &mut nb, base, bad);
            }
        }
    }
    // Sentinels: pass B reads up to N starts past the last one consumed, and one bad.
    sc.starts[ns..ns + N + 1].fill(u32::MAX);
    sc.bads[nb] = u32::MAX;

    // Pass B. Invariants: every start before `starts[k]` and every bad before `bads[bi]` lies in an
    // earlier line, so `starts[k] >= ls`; and `k <= ns`, `bi <= nb` (the sentinels stop both).
    let (starts, bads) = (&sc.starts[..ns + N + 1], &sc.bads[..nb + 1]);
    let (mut k, mut bi, mut ls) = (0usize, 0usize, 0u32);
    // SAFETY (every unchecked access below): `k + N <= ns + N < starts.len()` and
    // `bi < bads.len()` by the invariants.
    let start = |i: usize| unsafe { *starts.get_unchecked(i) };
    for &le in &sc.ends[..ne] {
        stats.lines += 1;
        // Exactly N field starts in [ls, le] and no quoting error: well-formed.
        if start(k + N - 1) <= le && start(k + N) > le && unsafe { *bads.get_unchecked(bi) } > le {
            let mut fields: [&[u8]; N] = [&[]; N];
            // A quoted field's start is one past its opening quote; its content ends one before
            // its separator, which is one before the next field's start (two if that is quoted).
            // The first field's start is `ls` or, quoted, `ls + 1`.
            let mut a = start(k);
            let mut q = a - ls;
            for (i, field) in fields.iter_mut().enumerate() {
                let (sep, next, qn) = if i + 1 < N {
                    let next = start(k + i + 1);
                    // SAFETY: ls <= a < next <= le < wlen + 1, so next - 1 is inside `w`.
                    let qn = (unsafe { *w.get_unchecked(next as usize - 1) } == b'"') as u32;
                    (next - 1 - qn, next, qn)
                } else {
                    (le, 0, 0)
                };
                debug_assert!(a <= sep - q && sep as usize <= wlen);
                // SAFETY: a <= sep - q (a quoted field holds at least its two quotes: a lone `"`
                // is a quoting error) and sep <= le <= wlen.
                *field = unsafe { w.get_unchecked(a as usize..(sep - q) as usize) };
                a = next;
                q = qn;
            }
            f(&fields);
            k += N;
        } else {
            stats.malformed += 1;
            while start(k) <= le {
                k += 1;
            }
            while unsafe { *bads.get_unchecked(bi) } <= le {
                bi += 1;
            }
        }
        ls = le + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect<const N: usize>(buf: &[u8], start: usize, end: usize) -> (Vec<Vec<Vec<u8>>>, LineStats) {
        let mut rows = Vec::new();
        let stats = for_each_line::<N, _>(buf, start, end, |fields| {
            rows.push(fields.iter().map(|f| f.to_vec()).collect());
        });
        (rows, stats)
    }

    fn reference<const N: usize>(text: &[u8]) -> (Vec<Vec<Vec<u8>>>, u64) {
        let mut rows = Vec::new();
        let mut malformed = 0;
        let body = text.strip_suffix(b"\n").unwrap_or(text);
        if text.is_empty() {
            return (rows, 0);
        }
        for line in body.split(|&c| c == b'\n') {
            match split_line::<N>(line) {
                Some(f) => rows.push(f.iter().map(|x| x.to_vec()).collect()),
                None => malformed += 1,
            }
        }
        (rows, malformed)
    }

    #[test]
    fn dialect_examples() {
        let text = b"a,b\n\"a\",\"\"\n\"a,b\"\n,\n\n\"\"\"\",x\n\"a\"\"b\",x\n \"u1\",x\n\"u1\" ,x\na,b,c\n\"\",\"z\"\r\nlast,line";
        let (rows, stats) = collect::<2>(text, 0, text.len());
        let want: Vec<Vec<Vec<u8>>> = vec![
            vec![b"a".to_vec(), b"b".to_vec()],
            vec![b"a".to_vec(), b"".to_vec()],
            vec![b"".to_vec(), b"".to_vec()],
            vec![b"last".to_vec(), b"line".to_vec()],
        ];
        assert_eq!(rows, want);
        assert_eq!(stats.lines, 12);
        assert_eq!(stats.malformed, 8);
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    fn random_text(r: &mut Rng, lines: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for _ in 0..lines {
            let fields = match r.below(10) {
                0 => r.below(10) as usize,
                _ => 4,
            };
            for i in 0..fields {
                if i > 0 {
                    out.push(b',');
                }
                let len = match r.below(8) {
                    0 => 0,
                    1 => 60 + r.below(80) as usize,
                    _ => r.below(12) as usize,
                };
                let quoted = r.below(5) == 0;
                if quoted {
                    out.push(b'"');
                }
                for _ in 0..len {
                    out.push(match r.below(40) {
                        0 => b'"',
                        1 => b'\r',
                        2 => b' ',
                        3 => 0xC3,
                        4 => b',',
                        _ => b'a' + r.below(26) as u8,
                    });
                }
                if quoted && r.below(10) != 0 {
                    out.push(b'"');
                }
            }
            out.push(b'\n');
        }
        out
    }

    #[test]
    fn many_windows_and_long_lines() {
        let mut r = Rng(0x0123_4567_89AB_CDEF);
        for round in 0..20usize {
            let mut text = random_text(&mut r, 3000);
            // Lines longer than a window, in the middle and at the end.
            for at in [text.len() / 3, text.len()] {
                let mut long = b"x,".to_vec();
                long.extend(std::iter::repeat_n(b'y', 20_000 + round * 100));
                long.extend_from_slice(b",z,w\n");
                let at = text[..at].iter().rposition(|&c| c == b'\n').map_or(0, |i| i + 1);
                text.splice(at..at, long);
            }
            if round % 2 == 0 {
                text.pop();
            }
            let (want, malformed) = reference::<4>(&text);
            let (got, stats) = collect::<4>(&text, 0, text.len());
            assert_eq!(got, want, "round {round}");
            assert_eq!(stats.malformed, malformed, "round {round}");
        }
    }

    /// Text from a tiny alphabet, so quotes, separators and empty fields and lines meet in every
    /// combination, including across 64-byte blocks.
    fn dense_text(r: &mut Rng, bytes: usize) -> Vec<u8> {
        (0..bytes).map(|_| [b'"', b'"', b',', b',', b'a', b'\n', b'\r'][r.below(7) as usize]).collect()
    }

    fn check_dense<const N: usize>(r: &mut Rng) {
        for round in 0..400 {
            let len = 1 + r.below(600) as usize;
            let mut text = dense_text(r, len);
            if round % 2 == 0 {
                text.push(b'\n');
            }
            let (want, malformed) = reference::<N>(&text);
            let (got, stats) = collect::<N>(&text, 0, text.len());
            assert_eq!(got, want, "N={N} round {round} {:?}", String::from_utf8_lossy(&text));
            assert_eq!(stats.malformed, malformed, "N={N} round {round}");
            assert_eq!(stats.lines as usize, got.len() + malformed as usize);
        }
    }

    #[test]
    fn quote_dense_text_matches_reference() {
        let mut r = Rng(0x5EED_0F_D15C_0BA1);
        check_dense::<2>(&mut r);
        check_dense::<4>(&mut r);
        check_dense::<7>(&mut r);
    }

    #[test]
    fn fast_path_matches_reference_on_random_text() {
        let mut r = Rng(0xDEAD_BEEF_1234_5678);
        for round in 0..300 {
            let mut text = random_text(&mut r, 200);
            if round % 3 == 0 {
                text.pop(); // no trailing newline on the last line
            }
            let (want, malformed) = reference::<4>(&text);
            let (got, stats) = collect::<4>(&text, 0, text.len());
            assert_eq!(got, want, "round {round}");
            assert_eq!(stats.malformed, malformed, "round {round}");
            // Padding after the buffer end must not change anything (the window reads ahead).
            let mut padded = text.clone();
            padded.extend_from_slice(&[b'\n'; 300]);
            let (got2, stats2) = collect::<4>(&padded, 0, text.len());
            assert_eq!(got2, want);
            assert_eq!(stats2.malformed, malformed);
        }
    }
}
