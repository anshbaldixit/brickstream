//! Exact 64-bit keys for IDs (event_id, click_id, user_id, product_id, campaign_id).
//!
//! **Packed keys.** An ID of 1..=9 bytes, every byte in 0x01..=0x7F, is packed 7 bits per byte into
//! a `u64`, the first byte in the highest bits (56..=62), unused positions zero. Bit 63 is 0.
//! The packing is injective (no byte is 0, so the padding can't be confused with data), and it is
//! order-preserving: comparing two packed keys as integers is the same as comparing the strings
//! byte by byte, a proper prefix sorting first. Every generated ID (at most 9 ASCII bytes) is packed,
//! and so are the fixtures' ` u1`, `A\tB` and `e3\r`.
//!
//! **Interned keys.** Any other non-empty ID (longer, non-ASCII, or holding a NUL byte) is interned:
//! the key is `1<<63 | shard<<32 | index` into a global table that stores each distinct byte string
//! once. Key equality is still exactly string equality, because a given string always gets the same
//! form (packed if it fits, otherwise its one interned index).
//!
//! **Ordering** must go through [`cmp_ids`], which compares bytes whenever either key is interned.
//! An interned key's integer value says nothing about its order.
//!
//! Key 0 never occurs (packed keys have a non-zero first byte; interned keys have bit 63), so 0 marks
//! an empty slot in every hash table. `FAST_FORCE_INTERN=1` interns every ID, which exercises the
//! fallback path on the fixtures and the dev set.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Mutex, OnceLock};

pub type Key = u64;

pub const INTERNED: u64 = 1 << 63;
const SHARD_BITS: u32 = 6;
const SHARDS: usize = 1 << SHARD_BITS;

static FORCE_INTERN: AtomicBool = AtomicBool::new(false);

pub fn set_force_intern(on: bool) {
    FORCE_INTERN.store(on, AtomicOrdering::Relaxed);
}

/// Murmur3's 64-bit finalizer. Used for partitioning (top bits) and hash tables (low bits).
#[inline(always)]
pub fn mix(key: u64) -> u64 {
    let mut h = key;
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^= h >> 33;
    h
}

/// The first `len` (0..=8) bytes of `b` as a little-endian u64, zero above. Reads only inside `b`.
#[inline(always)]
fn load_le(b: &[u8], len: usize) -> u64 {
    debug_assert!(len <= 8 && len <= b.len());
    if len >= 8 {
        u64::from_le_bytes(b[..8].try_into().unwrap())
    } else if len >= 4 {
        let lo = u32::from_le_bytes(b[..4].try_into().unwrap()) as u64;
        let hi = u32::from_le_bytes(b[len - 4..len].try_into().unwrap()) as u64;
        lo | (hi << (8 * (len - 4)))
    } else if len > 0 {
        let mut x = b[0] as u64;
        x |= (b[len / 2] as u64) << (8 * (len / 2));
        x |= (b[len - 1] as u64) << (8 * (len - 1));
        x
    } else {
        0
    }
}

const LOW7: u64 = 0x7F7F_7F7F_7F7F_7F7F;
const HIGH: u64 = 0x8080_8080_8080_8080;
const ONES: u64 = 0x0101_0101_0101_0101;

/// The packed key of `b`, or `None` if it doesn't fit (empty, over 9 bytes, a byte >= 0x80 or 0x00).
#[inline(always)]
pub fn pack(b: &[u8]) -> Option<Key> {
    let len = b.len();
    if len == 0 || len > 9 {
        return None;
    }
    let n8 = len.min(8);
    let lo = load_le(b, n8);
    let ninth = if len == 9 { b[8] as u64 } else { 0 };
    // Bytes past the ID are zero in `lo`; make them 0x01 for the zero-byte test.
    let pad = if n8 == 8 { 0 } else { ONES & !((1u64 << (8 * n8)) - 1) };
    let t = lo | pad;
    let has_zero = (t.wrapping_sub(ONES) & !t & HIGH) != 0;
    if (lo & HIGH) != 0 || has_zero || ninth >= 0x80 || (len == 9 && ninth == 0) {
        return None;
    }
    Some((pext7(lo.swap_bytes()) << 7) | ninth)
}

/// Gathers the low 7 bits of each byte of `x`, most significant byte first, into 56 bits.
#[inline(always)]
fn pext7(x: u64) -> u64 {
    #[cfg(all(target_arch = "x86_64", target_feature = "bmi2"))]
    {
        // SAFETY: the target feature is enabled at compile time.
        unsafe { core::arch::x86_64::_pext_u64(x, LOW7) }
    }
    #[cfg(not(all(target_arch = "x86_64", target_feature = "bmi2")))]
    {
        let mut out = 0u64;
        for i in (0..8).rev() {
            out = (out << 7) | ((x >> (8 * i)) & 0x7F);
        }
        out
    }
}

/// The key of a non-empty ID: packed if it fits (and interning isn't forced), else interned.
#[inline(always)]
pub fn key_of(b: &[u8]) -> Key {
    debug_assert!(!b.is_empty());
    if !FORCE_INTERN.load(AtomicOrdering::Relaxed) {
        if let Some(k) = pack(b) {
            return k;
        }
    }
    intern(b)
}

/// Decodes a packed key into `buf`; returns the length.
#[inline]
pub fn unpack(key: Key, buf: &mut [u8; 9]) -> usize {
    debug_assert!(key & INTERNED == 0);
    let mut n = 0;
    while n < 9 {
        let c = ((key >> (56 - 7 * n)) & 0x7F) as u8;
        if c == 0 {
            break;
        }
        buf[n] = c;
        n += 1;
    }
    n
}

struct Shard {
    map: HashMap<&'static [u8], u32>,
    strings: Vec<&'static [u8]>,
}

fn shards() -> &'static [Mutex<Shard>] {
    static SHARD_TABLE: OnceLock<Vec<Mutex<Shard>>> = OnceLock::new();
    SHARD_TABLE.get_or_init(|| {
        (0..SHARDS)
            .map(|_| Mutex::new(Shard { map: HashMap::new(), strings: Vec::new() }))
            .collect()
    })
}

fn bytes_hash(b: &[u8]) -> u64 {
    // FNV-1a: only used to pick a shard for the rare interned IDs.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &c in b {
        h ^= c as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Interns a non-empty byte string. The stored copy lives for the whole process.
#[cold]
pub fn intern(b: &[u8]) -> Key {
    let shard_id = (mix(bytes_hash(b)) >> (64 - SHARD_BITS)) as usize;
    let mut shard = shards()[shard_id].lock().unwrap();
    if let Some(&index) = shard.map.get(b) {
        return INTERNED | ((shard_id as u64) << 32) | index as u64;
    }
    let stored: &'static [u8] = Box::leak(b.to_vec().into_boxed_slice());
    let index = shard.strings.len() as u32;
    shard.strings.push(stored);
    shard.map.insert(stored, index);
    INTERNED | ((shard_id as u64) << 32) | index as u64
}

fn interned_bytes(key: Key) -> &'static [u8] {
    let shard_id = ((key >> 32) as usize) & (SHARDS - 1);
    let index = key as u32 as usize;
    shards()[shard_id].lock().unwrap().strings[index]
}

/// The bytes of any key, using `buf` for packed keys.
#[inline]
pub fn bytes_of(key: Key, buf: &mut [u8; 9]) -> &[u8] {
    if key & INTERNED == 0 {
        let n = unpack(key, buf);
        &buf[..n]
    } else {
        interned_bytes(key)
    }
}

/// Byte-wise order of the IDs behind two keys.
#[inline(always)]
pub fn cmp_ids(a: Key, b: Key) -> Ordering {
    if (a | b) & INTERNED == 0 {
        a.cmp(&b)
    } else {
        cmp_slow(a, b)
    }
}

#[cold]
fn cmp_slow(a: Key, b: Key) -> Ordering {
    let (mut x, mut y) = ([0u8; 9], [0u8; 9]);
    bytes_of(a, &mut x).cmp(bytes_of(b, &mut y))
}

/// Appends the ID's bytes to `out`.
#[inline]
pub fn push_bytes(key: Key, out: &mut Vec<u8>) {
    let mut buf = [0u8; 9];
    out.extend_from_slice(bytes_of(key, &mut buf));
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Random IDs biased toward the interesting cases: short, 9 vs 10 bytes, NUL, high bytes,
    /// spaces, tabs, `\r`, upper case, and multi-byte UTF-8.
    fn random_id(r: &mut Rng) -> Vec<u8> {
        let len = match r.below(10) {
            0 => 1 + r.below(3) as usize,
            1..=6 => 5 + r.below(5) as usize,
            7 => 9,
            8 => 10 + r.below(6) as usize,
            _ => 1 + r.below(20) as usize,
        };
        (0..len)
            .map(|_| match r.below(20) {
                0 => 0x00,
                1 => 0x80 + r.below(128) as u8,
                2 => b' ',
                3 => b'\t',
                4 => b'\r',
                5..=7 => b'A' + r.below(26) as u8,
                8..=10 => b'0' + r.below(10) as u8,
                11 => 0x01 + r.below(0x7F) as u8,
                _ => b'a' + r.below(26) as u8,
            })
            .collect()
    }

    fn reference_pack(b: &[u8]) -> Option<u64> {
        if b.is_empty() || b.len() > 9 || b.iter().any(|&c| c == 0 || c >= 0x80) {
            return None;
        }
        let mut k = 0u64;
        for i in 0..9 {
            k = (k << 7) | *b.get(i).unwrap_or(&0) as u64;
        }
        Some(k)
    }

    #[test]
    fn pack_matches_reference_and_roundtrips() {
        let mut r = Rng(0x9E37_79B9_7F4A_7C15);
        for _ in 0..200_000 {
            let id = random_id(&mut r);
            let packed = pack(&id);
            assert_eq!(packed, reference_pack(&id), "{id:?}");
            if let Some(k) = packed {
                assert_ne!(k, 0);
                assert_eq!(k & INTERNED, 0);
                let mut buf = [0u8; 9];
                assert_eq!(bytes_of(k, &mut buf), &id[..]);
            }
        }
    }

    #[test]
    fn fixture_ids_pack_and_sort() {
        for id in [&b" u1"[..], b"u1 ", b"A\tB", b"e3\r", b"user_id_a", b"u", b"\x7f\x7f\x7f\x7f\x7f\x7f\x7f\x7f\x7f"] {
            assert!(pack(id).is_some(), "{id:?}");
        }
        for id in [&b"0123456789"[..], "u\u{0662}".as_bytes(), b"a\0b"] {
            assert!(pack(id).is_none(), "{id:?}");
        }
        // Byte order, prefix first: k10 < k9, K5 < k1, k1 < k10; tab and space before letters.
        let order: [&[u8]; 8] = [b"\tz", b" u1", b"A\tB", b"K5", b"U1", b"k1", b"k10", b"k9"];
        for pair in order.windows(2) {
            assert!(pack(pair[0]).unwrap() < pack(pair[1]).unwrap(), "{:?} < {:?}", pair[0], pair[1]);
        }
    }

    #[test]
    fn mixed_keys_order_and_equality_by_bytes() {
        let mut r = Rng(12345);
        let ids: Vec<Vec<u8>> = (0..3000).map(|_| random_id(&mut r)).collect();
        // Every second ID goes through the interner even when it would pack, like FAST_FORCE_INTERN.
        let keys: Vec<Key> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| if i % 2 == 0 { intern(id) } else { key_of(id) })
            .collect();
        for i in 0..ids.len() {
            for j in (i..ids.len()).step_by(7) {
                assert_eq!(cmp_ids(keys[i], keys[j]), ids[i].cmp(&ids[j]), "{:?} vs {:?}", ids[i], ids[j]);
            }
        }
        // Interning is stable: the same bytes always get the same interned key.
        for (i, id) in ids.iter().enumerate().step_by(2) {
            assert_eq!(intern(id), keys[i]);
        }
        // A long (interned) ID sorts by bytes among packed ones, never after them all.
        let long = key_of(b"a_very_long_id_0001");
        assert_eq!(long & INTERNED, INTERNED);
        assert_eq!(cmp_ids(long, key_of(b"b")), Ordering::Less);
        assert_eq!(cmp_ids(key_of(b"Z"), long), Ordering::Less);
        assert_eq!(cmp_ids(key_of(b"a"), long), Ordering::Less);
    }
}
