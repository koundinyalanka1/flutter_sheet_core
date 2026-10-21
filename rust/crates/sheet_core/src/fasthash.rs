//! A small, fast hasher for maps keyed by cell positions.
//!
//! Recalculation keeps several maps of `(sheet, cell)` keys; the standard
//! library's SipHash is built to resist collision attacks on keys an
//! attacker chooses, which costs time here for no benefit (the keys are
//! small integers and a crafted file can only make a slow recalculation,
//! which the calculation deadline already bounds). This is the FxHash
//! scheme used by rustc, written in-tree rather than pulled in as a crate.

use std::hash::{BuildHasherDefault, Hasher};

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

#[derive(Default, Clone, Copy)]
pub struct FastHasher {
    hash: u64,
}

impl FastHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for FastHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for chunk in &mut chunks {
            let mut word = [0u8; 8];
            word.copy_from_slice(chunk);
            self.add(u64::from_le_bytes(word));
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut word = [0u8; 8];
            word[..rest.len()].copy_from_slice(rest);
            self.add(u64::from_le_bytes(word));
        }
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

pub type FastBuild = BuildHasherDefault<FastHasher>;
pub type FastMap<K, V> = std::collections::HashMap<K, V, FastBuild>;
pub type FastSet<K> = std::collections::HashSet<K, FastBuild>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_and_sets_work_with_cell_keys() {
        let mut map: FastMap<(usize, u32, u32), u32> = FastMap::default();
        let mut set: FastSet<(usize, u32, u32)> = FastSet::default();
        for s in 0..3usize {
            for r in 0..200u32 {
                for c in 0..20u32 {
                    map.insert((s, r, c), r * c);
                    set.insert((s, r, c));
                }
            }
        }
        assert_eq!(map.len(), 3 * 200 * 20);
        assert_eq!(set.len(), map.len());
        assert_eq!(map[&(2, 199, 19)], 199 * 19);
        assert!(!set.contains(&(3, 0, 0)));
    }

    #[test]
    fn byte_writes_cover_the_tail() {
        let mut a = FastHasher::default();
        a.write(b"abcdefghi");
        let mut b = FastHasher::default();
        b.write(b"abcdefghj");
        assert_ne!(a.finish(), b.finish());
    }
}
