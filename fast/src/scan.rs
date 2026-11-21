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

const WINDOW: usize = 128;

#[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
#[inline(always)]
fn masks(window: &[u8]) -> (u128, u128, u128) {
    use core::arch::x86_64::*;
    debug_assert!(window.len() >= WINDOW);
    // SAFETY: AVX2 is enabled at compile time and `window` has at least 128 readable bytes.
    unsafe {
        let p = window.as_ptr();
        let nl = _mm256_set1_epi8(b'\n' as i8);
        let cm = _mm256_set1_epi8(b',' as i8);
        let qt = _mm256_set1_epi8(b'"' as i8);
        let mut n = 0u128;
        let mut c = 0u128;
        let mut q = 0u128;
        for i in 0..4 {
            let v = _mm256_loadu_si256(p.add(32 * i) as *const __m256i);
            n |= (_mm256_movemask_epi8(_mm256_cmpeq_epi8(v, nl)) as u32 as u128) << (32 * i);
            c |= (_mm256_movemask_epi8(_mm256_cmpeq_epi8(v, cm)) as u32 as u128) << (32 * i);
            q |= (_mm256_movemask_epi8(_mm256_cmpeq_epi8(v, qt)) as u32 as u128) << (32 * i);
        }
        (n, c, q)
    }
}

#[cfg(not(all(target_arch = "x86_64", target_feature = "avx2")))]
#[inline(always)]
fn masks(window: &[u8]) -> (u128, u128, u128) {
    let (mut n, mut c, mut q) = (0u128, 0u128, 0u128);
    for (i, &b) in window[..WINDOW].iter().enumerate() {
        n |= ((b == b'\n') as u128) << i;
        c |= ((b == b',') as u128) << i;
        q |= ((b == b'"') as u128) << i;
    }
    (n, c, q)
}

/// Calls `f` with the N unquoted values of every well-formed line in `buf[start..end]`.
/// `start` must be a line start and `end` a line end (just after a `\n`, or the end of the file).
/// `buf` may extend past `end`; the fast path reads ahead inside `buf` but never splits past `end`.
#[inline(always)]
pub fn for_each_line<'a, const N: usize, F: FnMut(&[&'a [u8]; N])>(
    buf: &'a [u8],
    start: usize,
    end: usize,
    mut f: F,
) -> LineStats {
    let mut stats = LineStats::default();
    let mut pos = start;
    while pos < end {
        stats.lines += 1;
        if pos + WINDOW <= buf.len() {
            let (n, c, q) = masks(&buf[pos..pos + WINDOW]);
            if n != 0 {
                let len = n.trailing_zeros() as usize;
                // Chunks end just after a `\n` (or at the end of the file, where the window can't reach
                // past the buffer), so the first `\n` is never past `end`.
                debug_assert!(pos + len <= end);
                let below = (1u128 << len) - 1;
                let mut commas = c & below;
                if commas.count_ones() as usize != N - 1 {
                    stats.malformed += 1;
                    pos += len + 1;
                    continue;
                }
                let quotes = q & below;
                let mut fields: [&[u8]; N] = [&[]; N];
                let mut expected = 0u128;
                let mut s = 0usize;
                for (i, field) in fields.iter_mut().enumerate() {
                    let e = if i + 1 < N {
                        let p = commas.trailing_zeros() as usize;
                        commas &= commas - 1;
                        p
                    } else {
                        len
                    };
                    let (mut a, mut b) = (s, e);
                    if quotes != 0 && e >= s + 2 && (quotes >> s) & 1 == 1 && (quotes >> (e - 1)) & 1 == 1 {
                        expected |= (1u128 << s) | (1u128 << (e - 1));
                        a += 1;
                        b -= 1;
                    }
                    *field = &buf[pos + a..pos + b];
                    s = e + 1;
                }
                if quotes != expected {
                    stats.malformed += 1;
                } else {
                    f(&fields);
                }
                pos += len + 1;
                continue;
            }
        }
        // Slow path: a line longer than the window, or near the end of the buffer.
        let line_end = match memchr::memchr(b'\n', &buf[pos..end]) {
            Some(i) => pos + i,
            None => end,
        };
        match split_line::<N>(&buf[pos..line_end]) {
            Some(fields) => f(&fields),
            None => stats.malformed += 1,
        }
        pos = line_end + 1;
    }
    stats
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
