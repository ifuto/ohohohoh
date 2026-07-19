//! Phase 1/10 — hashbrown + FxHashMap helpers, packed positions, roaring dirty sets.

use roaring::RoaringBitmap;
use rustc_hash::FxHashMap;

#[inline]
pub fn pack_chunk_pos(cx: i32, cz: i32) -> i64 {
    ((cx as i64) << 32) ^ (cz as i32 as u32 as i64)
}

#[inline]
pub fn unpack_chunk_pos(p: i64) -> (i32, i32) {
    let cx = (p >> 32) as i32;
    let cz = p as i32;
    (cx, cz)
}

#[inline]
pub fn pack_block_pos(x: i32, y: i32, z: i32) -> i64 {
    let x = (x as i64) & 0x3ff_ffff; // 26
    let y = (y as i64) & 0xfff; // 12
    let z = (z as i64) & 0x3ff_ffff;
    x | (y << 26) | (z << 38)
}

/// Hot path map: FxHasher (rustc-hash) — faster for small keys.
pub type HotMap<K, V> = FxHashMap<K, V>;

/// Cold / large map: hashbrown default hasher with high load.
pub type ColdMap<K, V> = hashbrown::HashMap<K, V>;

#[derive(Debug, Default)]
pub struct DirtyBitset {
    pub bits: RoaringBitmap,
}

impl DirtyBitset {
    pub fn mark(&mut self, id: u32) {
        self.bits.insert(id);
    }

    pub fn clear(&mut self, id: u32) {
        self.bits.remove(id);
    }

    pub fn iter(&self) -> roaring::bitmap::Iter<'_> {
        self.bits.iter()
    }

    pub fn len(&self) -> u64 {
        self.bits.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_roundtrip() {
        let p = pack_chunk_pos(-3, 9);
        assert_eq!(unpack_chunk_pos(p), (-3, 9));
        let mut d = DirtyBitset::default();
        d.mark(10);
        assert_eq!(d.len(), 1);
    }
}
