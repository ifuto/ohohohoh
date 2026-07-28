
//! PSOキャッシュとPipelineLibrary - 初回起動ハング防止
//! ID3D12PipelineLibrary相当をRustで再現: PSOをハッシュ化してディスクキャッシュ。
//!
//! 【wave 163 FI (2026-07-28)】消費者: `FullGraphWiring` の PSO library
//! 計測 (`new`/`get`/`insert`/`save` = :1694-1699 実消費、`stats` = report
//! 実フィールド pso_lib_hits/pso_lib_misses)。捕捉 94 [小]: §7 消化 25 で
//! 消費者ゼロだった `stats()` へ読出し側を閉じた。捕捉 95 [小]: ワイヤ
//! blob 長 `as u32` 暗黙 wrap (rq fi_pso (3): 2^32 → 0) を `wire_blob_len`
//! 純粋関数の u32 飽和へ根治 (誠実注記: 実体 blob は MB 級で当該域は
//! 到達不能、契約の数学的完全性としての修正)。未捕捉棚卸し ([観]):
//! 保存ファイルは new() 注記通りロード不可の診断用アーティファクト、
//! HashMap 走査順由来でバイト列は実行間非決定 (12B レコード集合は同一、
//! 字节順のみ揺らぐ — I/O チャーンは 600 tick 間隔で実害極小のため
//! 仕様棚卸しに留める)。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::fs;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PsoKey {
    pub vs_hash: u64,
    pub ps_hash: u64,
    pub blend: u32,
    pub raster: u32,
    pub depth: u32,
}

pub struct PsoLibrary {
    cache: HashMap<PsoKey, Vec<u8>>,
    path: PathBuf,
    hits: u64,
    misses: u64,
}

/// wire 長フィールド値 (12B レコードの blob 長部分)。
/// 【wave 163 FI 捕捉 95】契約の純粋関数化: 4GiB 超は u32 飽和。
/// (実体 PSO blob は MB 級で到達不能だが、キャスト契約は wrap 禁止族一
/// (capture 84/88) として数学的に保証する)。
fn wire_blob_len(len: usize) -> u32 {
    (len as u64).min(u32::MAX as u64) as u32
}

impl PsoLibrary {
    pub fn new(cache_dir: &Path) -> Self {
        let path = cache_dir.join("rsift_pso_cache.bin");
        // 監査 (2026-07-22): save() のワイヤ形式は (vs_hash, blob 長) の 12B
        // レコード列で、blob 本体・ps_hash 等は含まないため全エントリの復元は
        // 不可能 = ロードは意図的に行わない。旧コードはファイルを読み捨てた
        // うえで「キャッシュがあればヒットとして扱う」と虚偽コメントを残して
        // いたため撤去 (実態: 起動ごとに空キャッシュから始まり、セッション内
        // で PSO 再コンパイルをインメモリ集約する設計)。
        let cache = HashMap::new();
        Self { cache, path, hits: 0, misses: 0 }
    }

    pub fn get(&mut self, key: &PsoKey) -> Option<&Vec<u8>> {
        if let Some(v) = self.cache.get(key) {
            self.hits += 1;
            Some(v)
        } else {
            self.misses += 1;
            None
        }
    }

    pub fn insert(&mut self, key: PsoKey, blob: Vec<u8>) {
        self.cache.insert(key, blob);
    }

    pub fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = Vec::new();
        for (k, v) in &self.cache {
            out.extend_from_slice(&k.vs_hash.to_le_bytes());
            // 【wave 163 FI 捕捉 95】旧 `v.len() as u32` の暗黙 wrap を
            // 飽和契約の純粋関数へ (capture 84/88 同型)。
            out.extend_from_slice(&wire_blob_len(v.len()).to_le_bytes());
        }
        fs::write(&self.path, out)
    }

    pub fn stats(&self) -> (u64, u64) { (self.hits, self.misses) }
}

#[cfg(test)]
mod strict_tests {
    use super::*;

    fn key(vs: u64) -> PsoKey {
        PsoKey { vs_hash: vs, ps_hash: 1, blend: 2, raster: 3, depth: 4 }
    }

    fn unique_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rsift_pso_strict_{}_{}_{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn miss_then_hit_accounting_and_overwrite() {
        let dir = unique_dir("acct");
        let mut lib = PsoLibrary::new(&dir);
        assert_eq!(lib.stats(), (0, 0));
        assert!(lib.get(&key(1)).is_none(), "未登録キーは miss");
        assert_eq!(lib.stats(), (0, 1));
        lib.insert(key(1), vec![9, 9, 9]);
        assert_eq!(lib.get(&key(1)), Some(&vec![9, 9, 9]), "挿入直後にヒット");
        assert_eq!(lib.stats(), (1, 1));
        // 同一キー再挿入は blob 上書き ( hits/misses は get のみが進める )
        lib.insert(key(1), vec![7]);
        assert_eq!(lib.get(&key(1)), Some(&vec![7]));
        assert_eq!(lib.stats(), (2, 1));
    }

    #[test]
    fn save_wire_format_is_hash_and_len_records() {
        let dir = unique_dir("wire");
        let mut lib = PsoLibrary::new(&dir);
        lib.insert(key(0x1122_3344_5566_7788), vec![10, 20, 30]);
        lib.insert(key(0x0102_0304_0506_0708), Vec::new());
        lib.save().expect("save");
        let raw = fs::read(dir.join("rsift_pso_cache.bin")).expect("cache file");
        // 仕様: 1 レコード = vs_hash LE (8B) + blob 長 LE (4B) の 12B。
        // ps_hash/blend/raster/depth と blob 本体は保存されない (ロード不可の
        // 理由。上記 new() の監査注記を参照)。
        assert_eq!(raw.len(), 24, "2 エントリ x 12B");
        let mut recs: Vec<(u64, u32)> = raw
            .chunks_exact(12)
            .map(|c| {
                (
                    u64::from_le_bytes(c[..8].try_into().unwrap()),
                    u32::from_le_bytes(c[8..12].try_into().unwrap()),
                )
            })
            .collect();
        recs.sort_unstable(); // HashMap 走査順は非決定的なので集合として照合
        assert_eq!(
            recs,
            vec![(0x0102_0304_0506_0708_u64, 0_u32), (0x1122_3344_5566_7788_u64, 3_u32)]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_creates_parent_dirs() {
        let dir = unique_dir("nested").join("a").join("b");
        let mut lib = PsoLibrary::new(&dir);
        lib.insert(key(42), vec![1]);
        lib.save().expect("nested save");
        let raw = fs::read(dir.join("rsift_pso_cache.bin")).expect("written");
        assert_eq!(raw.len(), 12, "1 エントリ 12B");
        // dir = <unique>/a/b → ancestors().nth(2) でユニーク親ごと掃除
        let _ = fs::remove_dir_all(dir.ancestors().nth(2).unwrap().to_path_buf());
    }

    /// 【wave 163 FI 捕捉 95】wire 長フィールドは u32 飽和であること。
    /// 旧 `v.len() as u32` は 4GiB 超で暗黙 wrap (capture 84/88 同型、
    /// rq fi_pso (3): 2^32 → 0)。4GiB alloc 不要の契約 pin (純粋関数化)。
    #[test]
    fn wire_blob_len_saturates_past_u32() {
        assert_eq!(wire_blob_len(64), 64);
        assert_eq!(wire_blob_len(u32::MAX as usize), u32::MAX);
        assert_eq!(
            wire_blob_len(u32::MAX as usize + 1),
            u32::MAX,
            "wrap せず飽和"
        );
    }

    #[test]
    fn preexisting_disk_blob_is_ignored_by_design() {
        let dir = unique_dir("junk");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("rsift_pso_cache.bin"), vec![0xAB; 64]).unwrap();
        let mut lib = PsoLibrary::new(&dir); // パニックしない・復元もしない (仕様)
        assert_eq!(lib.stats(), (0, 0));
        assert!(lib.get(&key(0xAB)).is_none(), "ディスク上の blob は復元されない (miss)");
        assert_eq!(lib.stats(), (0, 1));
        let _ = fs::remove_dir_all(&dir);
    }
}
