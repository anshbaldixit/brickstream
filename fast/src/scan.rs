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
const NL: u32 = 1 << 31;

/// Per-thread buffers for one window's structural index.
struct Scratch {
    /// Offsets of the window's separators (`,` and `\n`), `NL` set on newlines.
    pos: Vec<u32>,
    /// Quote bitmask of each 64-byte block, and the number of quotes before it.
    qm: Vec<u64>,
    qpre: Vec<u32>,
}

thread_local! {
    static SCRATCH: RefCell<Scratch> = RefCell::new(Scratch {
        pos: vec![0; WINDOW + 128],
        qm: vec![0; WINDOW / 64 + 2],
        qpre: vec![0; WINDOW / 64 + 2],
    });
}

/// Calls `f` with the N unquoted values of every well-formed line in `buf[start..end]`.
/// `start` must be a line start and `end` a line end (just after a `\n`, or the end of the file).
///
/// The bytes are processed in windows of whole lines. Pass A builds the window's structural index:
/// AVX2 compares give `\n`, `,` and `"` bitmasks per 64 bytes, separator positions are flattened from
/// the masks almost without branches, and quote counts are kept per block. Pass B walks the lines:
/// a line is well-formed iff its first N-1 separators are commas and the N-th is its newline, and its
/// quote count (from the per-block prefix counts) equals twice the number of its fields that are at
/// least 2 bytes long and start and end with `"` (those quotes are then exactly the brackets).
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
    let nblocks = wlen.div_ceil(64);
    // Pass A: the structural index.
    let mut npos = 0usize;
    let mut quotes = 0u32;
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
        }
        // SAFETY: b < nblocks <= WINDOW / 64 + 1 < qm.len() == qpre.len().
        unsafe {
            *sc.qm.get_unchecked_mut(b) = qt;
            *sc.qpre.get_unchecked_mut(b) = quotes;
        }
        quotes += qt.count_ones();
        let mut seps = nl | cm;
        let cnt = seps.count_ones() as usize;
        let base = (64 * b) as u32;
        debug_assert!(npos + 64 <= sc.pos.len());
        // SAFETY: npos <= 64 * b and pos has WINDOW + 128 entries, so npos + 64 is in bounds.
        unsafe {
            let dst = sc.pos.as_mut_ptr().add(npos);
            // Eight positions unconditionally (most blocks have fewer), the rest in a rare loop.
            for j in 0..8 {
                let tz = seps.trailing_zeros() & 63;
                *dst.add(j) = (base + tz) | ((((nl >> tz) & 1) as u32) << 31);
                seps &= seps.wrapping_sub(1);
            }
            let mut j = 8;
            while j < cnt {
                let tz = seps.trailing_zeros();
                *dst.add(j) = (base + tz) | ((((nl >> tz) & 1) as u32) << 31);
                seps &= seps.wrapping_sub(1);
                j += 1;
            }
        }
        npos += cnt;
    }
    sc.qm[nblocks] = 0;
    sc.qpre[nblocks] = quotes;
    let (qm, qpre) = (&sc.qm, &sc.qpre);
    let qcount = |p: u32| -> u32 {
        let b = (p >> 6) as usize;
        debug_assert!(b <= nblocks);
        // SAFETY: p <= wlen, so b <= nblocks, and entries 0..=nblocks were written above.
        unsafe { qpre.get_unchecked(b) + (qm.get_unchecked(b) & ((1u64 << (p & 63)) - 1)).count_ones() }
    };
    // Pass B: the lines.
    let pos = &sc.pos[..npos];
    let mut k = 0usize;
    let mut ls = 0u32;
    while k < npos {
        if k + N <= npos {
            let mut early = 0u32;
            // SAFETY: k + N <= npos.
            let last = unsafe {
                for i in 0..N - 1 {
                    early |= *pos.get_unchecked(k + i);
                }
                *pos.get_unchecked(k + N - 1)
            };
            if early & NL == 0 && last & NL != 0 {
                let le = last & !NL;
                stats.lines += 1;
                // SAFETY: k + N - 1 < npos.
                emit::<N, F>(w, ls, unsafe { pos.get_unchecked(k..k + N - 1) }, le, qcount(le) - qcount(ls), f, stats);
                ls = le + 1;
                k += N;
                continue;
            }
        }
        // Not N-1 commas and then a newline: skip to this line's newline.
        let mut kn = k;
        while kn < npos && pos[kn] & NL == 0 {
            kn += 1;
        }
        if kn == npos {
            break; // the file's last line has no `\n`
        }
        stats.lines += 1;
        stats.malformed += 1;
        ls = (pos[kn] & !NL) + 1;
        k = kn + 1;
    }
    if (ls as usize) < wlen {
        // The file's last line without a trailing `\n`: its separators are pos[k..], all commas.
        stats.lines += 1;
        if npos - k == N - 1 {
            emit::<N, F>(w, ls, &pos[k..npos], wlen as u32, qcount(wlen as u32) - qcount(ls), f, stats);
        } else {
            stats.malformed += 1;
        }
    }
}

#[inline(always)]
fn emit<'a, const N: usize, F: FnMut(&[&'a [u8]; N])>(
    w: &'a [u8],
    ls: u32,
    commas: &[u32],
    le: u32,
    quotes: u32,
    f: &mut F,
    stats: &mut LineStats,
) {
    debug_assert!(commas.len() == N - 1);
    let mut fields: [&[u8]; N] = [&[]; N];
    let mut s = ls as usize;
    // SAFETY: the separators are increasing offsets inside the window and le <= w.len(), so every
    // range s..e below is inside w (s is one past the previous separator, at most e).
    // Branch-free bracket check (a fifth of all fields are quoted, at random): a field of at least
    // 2 bytes that starts and ends with `"` loses one byte at each end.
    let last = w.len() - 1;
    let mut bracketed = 0u32;
    for (i, field) in fields.iter_mut().enumerate() {
        let e = if i + 1 < N { unsafe { *commas.get_unchecked(i) as usize } } else { le as usize };
        debug_assert!(s <= e && e <= w.len() && !w.is_empty());
        // SAFETY: both indexes are clamped into the window (non-empty: it holds this line).
        let (first, final_) = unsafe { (*w.get_unchecked(s.min(last)), *w.get_unchecked(e.saturating_sub(1).min(last))) };
        let q = ((e >= s + 2) & (first == b'"') & (final_ == b'"')) as usize;
        bracketed += q as u32;
        // SAFETY: s + q <= e - q <= w.len() (q = 1 only if e >= s + 2).
        *field = unsafe { w.get_unchecked(s + q..e - q) };
        s = e + 1;
    }
    if quotes != 2 * bracketed {
        stats.malformed += 1;
        return;
    }
    f(&fields);
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
