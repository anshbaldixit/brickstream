//! Open-addressing hash tables keyed by exact u64 ID keys (key 0 = empty slot; see key.rs).

use crate::key::mix;

/// A read-only map from key to a u32 value, built once (products, campaigns).
pub struct KeyMap {
    slots: Vec<(u64, u32)>,
    mask: usize,
}

impl KeyMap {
    /// Builds the map; for a repeated key the first entry wins.
    pub fn new(entries: &[(u64, u32)]) -> Self {
        let cap = (entries.len() * 2).next_power_of_two().max(16);
        let mut slots = vec![(0u64, 0u32); cap];
        let mask = cap - 1;
        for &(k, v) in entries {
            debug_assert!(k != 0);
            let mut i = mix(k) as usize & mask;
            loop {
                if slots[i].0 == 0 {
                    slots[i] = (k, v);
                    break;
                }
                if slots[i].0 == k {
                    break;
                }
                i = (i + 1) & mask;
            }
        }
        KeyMap { slots, mask }
    }

    #[inline(always)]
    pub fn get(&self, k: u64) -> Option<u32> {
        let mut i = mix(k) as usize & self.mask;
        loop {
            let (sk, v) = self.slots[i];
            if sk == k {
                return Some(v);
            }
            if sk == 0 {
                return None;
            }
            i = (i + 1) & self.mask;
        }
    }
}
