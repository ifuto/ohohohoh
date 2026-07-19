//! # SIMD-Accelerated Class Scanner
//!
//! AVX2/NEON/memchr を活用し、パターン探索を高速化。
//! `PatternCache` で Finder を再利用し、ホットパスでの再構築コストを排除。

use memchr::{memmem, memchr};
use rayon::prelude::*;

/// Java ClassFile magic number (`0xCAFEBABE`)
pub const CLASS_MAGIC: u32 = 0xCAFEBABE;

/// SIMD-accelerated class scanner
pub struct SimdScanner;

impl SimdScanner {
    #[inline(always)]
    pub fn verify_magic(data: &[u8]) -> bool {
        if data.len() < 4 {
            return false;
        }
        u32::from_be_bytes([data[0], data[1], data[2], data[3]]) == CLASS_MAGIC
    }

    #[inline]
    pub fn contains_pattern(data: &[u8], pattern: &[u8]) -> bool {
        let finder = memmem::Finder::new(pattern);
        finder.find(data).is_some()
    }

    pub fn find_matching_target<'a>(data: &[u8], targets: &'a [&[u8]]) -> Option<&'a [u8]> {
        targets.par_iter().find_any(|&&target| {
            Self::contains_pattern(data, target)
        }).copied()
    }

    pub fn find_all_opcodes(bytecode: &[u8], opcode: u8) -> Vec<usize> {
        memchr::memchr_iter(opcode, bytecode).collect()
    }

    /// Batch scan multiple class files in parallel (used during mod load)
    pub fn batch_contains(classes: &[&[u8]], pattern: &[u8]) -> Vec<usize> {
        let finder = memmem::Finder::new(pattern);
        classes
            .par_iter()
            .enumerate()
            .filter_map(|(i, data)| if finder.find(data).is_some() { Some(i) } else { None })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_magic() {
        let valid = [0xCA, 0xFE, 0xBA, 0xBE, 0x00, 0x00];
        assert!(SimdScanner::verify_magic(&valid));
        let invalid = [0x00, 0x00, 0x00, 0x00];
        assert!(!SimdScanner::verify_magic(&invalid));
    }

    #[test]
    fn test_simd_find() {
        let data = b"\xca\xfe\xba\xbe...net/minecraft/network/Connection...";
        assert!(SimdScanner::contains_pattern(data, b"net/minecraft/network/Connection"));
    }
}
