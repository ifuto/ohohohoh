//! # 32. Encoding Occupancy in Memory Location (`LocationEncodedOccupancy` - 2025 CGF)
//!
//! SVDAG のノードデータ自体の格納アドレスに、ボクセルジオメトリの構造情報をエンコードする。
//! ノードのポインタやフラグに使われていたビットを、メモリアドレスの配置規則 (`address & 0x7`) に
//! 暗黙的に埋め込むことで、明示的な子マスクストレージを削減する。

pub struct LocationEncodedOccupancy {
    /// Memory pool where node alignment modulo 8 represents its active child count or type.
    pub pool_bytes: Vec<u64>,
}

impl Default for LocationEncodedOccupancy {
    fn default() -> Self {
        Self::new()
    }
}

impl LocationEncodedOccupancy {
    pub fn new() -> Self {
        Self {
            pool_bytes: Vec::with_capacity(8192),
        }
    }

    /// Allocate a node such that its `index % 8 == occupancy_tag`.
    pub fn allocate_tagged_node(&mut self, occupancy_tag: u8, payload: u64) -> usize {
        let tag = (occupancy_tag & 0x7) as usize;
        while self.pool_bytes.len() % 8 != tag {
            self.pool_bytes.push(0); // padding dummy word to hit exact alignment tag
        }
        let idx = self.pool_bytes.len();
        self.pool_bytes.push(payload);
        idx
    }

    /// Decode the implicit occupancy tag solely from the pointer/index address.
    #[inline(always)]
    pub fn decode_occupancy_from_location(index: usize) -> u8 {
        (index % 8) as u8
    }

    pub fn get_payload(&self, index: usize) -> u64 {
        self.pool_bytes.get(index).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_location_encoded_occupancy() {
        let mut leo = LocationEncodedOccupancy::new();
        let idx = leo.allocate_tagged_node(5, 0x12345678);
        assert_eq!(LocationEncodedOccupancy::decode_occupancy_from_location(idx), 5);
        assert_eq!(leo.get_payload(idx), 0x12345678);
    }
}
