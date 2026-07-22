//! Per-chunk frame-to-frame reuse — skip prepare/RLE/mesh/SVO when terrain unchanged.
//!
//! Telemetry (`FrameReuseStats`) lets us see whether reuse helps (high hit rate, low
//! memory) or hurts (evictions, stale rebuilds, RAM bloat).

use crate::chunk_mesh::BuiltChunkMesh;
use crate::lod_hybrid::{LodHybridSelector, LodTier};
use crate::section_rle::RleSection;
use crate::svo::SparseVoxelOctree;
use std::collections::HashMap;
use tracing::debug;

const DEFAULT_MAX_ENTRIES: usize = 512;
/// Frames without touch before LRU eviction.
const STALE_FRAMES: u64 = 120;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ReuseEncoding {
    #[default]
    None,
    Mesh,
    Svo,
}

#[derive(Debug, Clone, Default)]
pub struct FrameReuseStats {
    pub hits: u32,
    pub misses: u32,
    pub lod_re_simplify: u32,
    pub stale_invalidations: u32,
    pub evictions: u32,
    pub memory_bytes: u64,
    /// Rough CPU work units avoided (prepare + mesh or SVO build).
    pub work_units_saved: u64,
}

impl FrameReuseStats {
    pub fn hit_rate(&self) -> f32 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f32 / total as f32
        }
    }

    /// Heuristic verdict for live tuning.
    pub fn verdict(&self) -> &'static str {
        let rate = self.hit_rate();
        let mb = self.memory_bytes as f64 / (1024.0 * 1024.0);
        if rate >= 0.85 && mb < 64.0 {
            "GOOD — high reuse, bounded RAM"
        } else if rate >= 0.50 && mb < 128.0 {
            "OK — moderate reuse, watch memory"
        } else if mb >= 128.0 {
            "BAD — memory pressure; shrink cache or disable"
        } else if rate < 0.30 {
            "WEAK — low hit rate; camera/world churn or cold start"
        } else {
            "MIXED — tune max_entries or stale window"
        }
    }
}

#[derive(Debug, Clone)]
struct ChunkFrameEntry {
    // 注: 旧 `fingerprint` (変更検知ハッシュのつもり) と `sections` (パレット全列の
    // 複製 = エントリ当たり数百KBの二重保持) は一度も読まれないデッド状態だった。
    // 鮮度判定は実際には last_tick ベース (2026-07-21 監査でメモリ二重保持も解消)。
    rle: Vec<RleSection>,
    occupied: Vec<u32>,
    /// Full-detail mesh before LOD thinning (Near tier).
    full_mesh: Option<BuiltChunkMesh>,
    svo: Option<SparseVoxelOctree>,
    encoding: ReuseEncoding,
    last_tick: u64,
    memory_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct ReusedChunk {
    pub mesh: BuiltChunkMesh,
    pub rle: Vec<RleSection>,
    pub occupied: Vec<u32>,
    pub encoding: ReuseEncoding,
}

pub struct FrameReuseCache {
    enabled: bool,
    max_entries: usize,
    tick: u64,
    entries: HashMap<(i32, i32), ChunkFrameEntry>,
    pub stats: FrameReuseStats,
    cumulative_hits: u64,
    cumulative_misses: u64,
}

impl FrameReuseCache {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            max_entries: DEFAULT_MAX_ENTRIES,
            tick: 0,
            entries: HashMap::new(),
            stats: FrameReuseStats::default(),
            cumulative_hits: 0,
            cumulative_misses: 0,
        }
    }

    pub fn adaptive(_feather_enabled: bool) -> Self {
        // Investigation phase: always on so we can read hit_rate in logs on all tiers.
        Self::new(true)
    }

    pub fn begin_frame(&mut self, tick: u64) {
        self.tick = tick;
        self.stats = FrameReuseStats::default();
    }

    /// エントリが**実際に保持する**バイト数。
    /// (旧実装は wave 2 で除去済みの `sections` 複製 (~8KB/section) を幽霊計上し、
    /// 実際に保持する `occupied` 列を未計上だった — 2026-07-22 wave 18 で実量に整合)
    fn entry_bytes(
        rle: &[RleSection],
        occupied: &[u32],
        mesh: Option<&BuiltChunkMesh>,
        svo: Option<&SparseVoxelOctree>,
    ) -> usize {
        let rle_bytes: usize = rle.iter().map(|s| s.runs.len() * 4).sum();
        let occupied_bytes = occupied.len() * 4;
        let mesh_bytes = mesh
            .map(|m| m.vertices.len() * 12 + m.indices.len() * 4)
            .unwrap_or(0);
        let svo_bytes = svo.map(|s| s.memory_bytes()).unwrap_or(0);
        rle_bytes + occupied_bytes + mesh_bytes + svo_bytes
    }

    /// Try reuse without re-running prepare/mesh/SVO. Returns None on miss.
    ///
    /// **統計契約**: hits + misses == 試行回数 (全 None return 経路は必ず
    /// misses (+cumulative_misses) を 1 回だけ計上する。呼出側で追加の
    /// ミス計上をすると二重計上になる — 独自のバイパス経路に限り
    /// `record_miss` を使うこと)。
    pub fn try_reuse(
        &mut self,
        cx: i32,
        cz: i32,
        dist_blocks: f32,
        lod: &LodHybridSelector,
    ) -> Option<ReusedChunk> {
        if !self.enabled {
            return None;
        }
        let key = (cx, cz);
        let entry = match self.entries.get_mut(&key) {
            Some(e) => e,
            None => {
                self.stats.misses += 1;
                self.cumulative_misses += 1;
                return None;
            }
        };
        let tier = lod.tier_for_distance(dist_blocks);
        let want_svo = lod.use_svo_encoding(tier);

        if want_svo {
            if entry.encoding != ReuseEncoding::Svo || entry.svo.is_none() {
                self.stats.stale_invalidations += 1;
                self.stats.misses += 1;
                self.cumulative_misses += 1;
                return None;
            }
            entry.last_tick = self.tick;
            self.stats.hits += 1;
            self.cumulative_hits += 1;
            self.stats.work_units_saved += 3;
            return Some(ReusedChunk {
                mesh: BuiltChunkMesh {
                    chunk_x: cx,
                    chunk_z: cz,
                    vertices: vec![],
                    indices: vec![],
                    is_empty: true,
                },
                rle: entry.rle.clone(),
                occupied: entry.occupied.clone(),
                encoding: ReuseEncoding::Svo,
            });
        }

        // Mesh 要求なのに full_mesh を持たない (例: 遠方で Svo 格納された
        // チャンクに接近して Mesh tier へ移行した)。旧実装は `?` による
        // **無計上のサイレントミス**で、hit_rate を偽装していた (wave 18)。
        let full = match entry.full_mesh.as_ref() {
            Some(f) => f.clone(),
            None => {
                self.stats.stale_invalidations += 1;
                self.stats.misses += 1;
                self.cumulative_misses += 1;
                return None;
            }
        };
        let mesh = if tier == LodTier::Near {
            full
        } else {
            self.stats.lod_re_simplify += 1;
            lod.simplify_mesh(full, tier)
        };
        entry.last_tick = self.tick;
        self.stats.hits += 1;
        self.cumulative_hits += 1;
        self.stats.work_units_saved += 2;
        Some(ReusedChunk {
            mesh,
            rle: entry.rle.clone(),
            occupied: entry.occupied.clone(),
            encoding: ReuseEncoding::Mesh,
        })
    }

    /// `try_reuse` を**バイパス**する独自ミス経路の帳簿用。
    /// `try_reuse` は全ミス経路を内部計上済みなので、成功/失敗のいずれでも
    /// 呼出側がこれを追加実行すると **二重計上** になる (wave 18 で
    /// render_pipeline の冗長呼出を除去した経緯あり)。
    pub fn record_miss(&mut self) {
        self.stats.misses += 1;
        self.cumulative_misses += 1;
    }

    pub fn store(
        &mut self,
        cx: i32,
        cz: i32,
        rle: &[RleSection],
        occupied: &[u32],
        full_mesh: Option<BuiltChunkMesh>,
        svo: Option<SparseVoxelOctree>,
        encoding: ReuseEncoding,
    ) {
        if !self.enabled {
            return;
        }
        let memory_bytes = Self::entry_bytes(rle, occupied, full_mesh.as_ref(), svo.as_ref());
        let entry = ChunkFrameEntry {
            rle: rle.to_vec(),
            occupied: occupied.to_vec(),
            full_mesh,
            svo,
            encoding,
            last_tick: self.tick,
            memory_bytes,
        };
        self.entries.insert((cx, cz), entry);
        self.enforce_capacity();
        self.refresh_memory_stat();
    }

    /// Invalidate one chunk when JNI block updates land.
    pub fn invalidate(&mut self, cx: i32, cz: i32) {
        if self.entries.remove(&(cx, cz)).is_some() {
            self.stats.stale_invalidations += 1;
            self.refresh_memory_stat();
        }
    }

    pub fn end_frame(&mut self) {
        let stale_before = self.tick.saturating_sub(STALE_FRAMES);
        let stale_keys: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, e)| e.last_tick < stale_before)
            .map(|(k, _)| *k)
            .collect();
        for k in stale_keys {
            self.entries.remove(&k);
            self.stats.evictions += 1;
        }
        self.enforce_capacity();
        self.refresh_memory_stat();
    }

    fn enforce_capacity(&mut self) {
        while self.entries.len() > self.max_entries {
            // (last_tick, key) でタイブレーク: 同 tick (同フレーム一括格納で頻発)
            // の最小を HashMap 反復順に委ねると**追い出し結果がプロセス毎に
            // 非決定**になる (RandomState 由来)。キー座標辞書順の厳密最小で
            // 完全決定的にする (wave 18)。
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(k, e)| (e.last_tick, **k))
                .map(|(k, _)| *k);
            if let Some(k) = oldest {
                self.entries.remove(&k);
                self.stats.evictions += 1;
            } else {
                break;
            }
        }
    }

    fn refresh_memory_stat(&mut self) {
        self.stats.memory_bytes = self.entries.values().map(|e| e.memory_bytes as u64).sum();
    }

    pub fn cumulative_hit_rate(&self) -> f32 {
        let t = self.cumulative_hits + self.cumulative_misses;
        if t == 0 {
            0.0
        } else {
            self.cumulative_hits as f32 / t as f32
        }
    }

    pub fn log_report(&self, tick: u64) {
        debug!(
            "[FrameReuse] tick={} hits={} misses={} lod_resimplify={} evict={} hit_rate={:.0}% cum={:.0}% mem={:.1}MB saved_work={} verdict={}",
            tick,
            self.stats.hits,
            self.stats.misses,
            self.stats.lod_re_simplify,
            self.stats.evictions,
            self.stats.hit_rate() * 100.0,
            self.cumulative_hit_rate() * 100.0,
            self.stats.memory_bytes as f64 / (1024.0 * 1024.0),
            self.stats.work_units_saved,
            self.stats.verdict(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary_greedy_meshing::{demo_column_palettes, mesh_chunk_column, SectionPalette};
    use crate::chunk_mesh::Quantized12ByteVertex;
    use crate::section_rle::{occupied_section_indices, RleRun};
    use crate::svo::SparseVoxelOctree;

    fn build_entry(cx: i32, cz: i32) -> (Vec<SectionPalette>, Vec<RleSection>, BuiltChunkMesh, Vec<u32>) {
        let sections = demo_column_palettes(cx, cz);
        let rle: Vec<RleSection> = sections.iter().map(RleSection::encode).collect();
        let occupied = occupied_section_indices(&rle);
        let mesh = mesh_chunk_column(&sections, cx, cz, true);
        (sections, rle, mesh, occupied)
    }

    fn store_for_dist(
        cache: &mut FrameReuseCache,
        cx: i32,
        cz: i32,
        dist: f32,
        lod: &LodHybridSelector,
    ) {
        let (sections, rle, mesh, occupied) = build_entry(cx, cz);
        let tier = lod.tier_for_distance(dist);
        if lod.use_svo_encoding(tier) {
            let svo = SparseVoxelOctree::from_column(&sections);
            cache.store(cx, cz, &rle, &occupied, None, Some(svo), ReuseEncoding::Svo);
        } else {
            cache.store(
                cx,
                cz,
                &rle,
                &occupied,
                Some(mesh),
                None,
                ReuseEncoding::Mesh,
            );
        }
    }

    #[test]
    fn second_frame_hits_for_static_column() {
        let mut cache = FrameReuseCache::new(true);
        let lod = LodHybridSelector::new(true, true, true);
        let dist = 32.0; // Near tier → mesh encoding

        cache.begin_frame(1);
        store_for_dist(&mut cache, 2, 3, dist, &lod);
        cache.end_frame();

        cache.begin_frame(2);
        let reused = cache.try_reuse(2, 3, dist, &lod);
        assert!(reused.is_some());
        assert_eq!(cache.stats.hits, 1);
        assert!(cache.stats.hit_rate() > 0.9);
    }

    #[test]
    fn orbit_simulation_high_cumulative_hit_rate() {
        let mut cache = FrameReuseCache::new(true);
        let lod = LodHybridSelector::new(true, true, true);
        let coords: Vec<(i32, i32)> = (-6..=6)
            .flat_map(|x| (-6..=6).map(move |z| (x, z)))
            .collect();

        for tick in 1..=300u64 {
            cache.begin_frame(tick);
            let mut frame_hits = 0u32;
            for &(cx, cz) in &coords {
                let dist = (cx as f32).hypot(cz as f32) * 16.0;
                if cache.try_reuse(cx, cz, dist, &lod).is_some() {
                    frame_hits += 1;
                    continue;
                }
                store_for_dist(&mut cache, cx, cz, dist, &lod);
            }
            cache.end_frame();
            if tick == 2 {
                assert!(
                    frame_hits > coords.len() as u32 / 2,
                    "frame 2 should mostly hit after warm-up"
                );
            }
        }
        assert!(
            cache.cumulative_hit_rate() > 0.95,
            "cumulative hit rate {:.1}%",
            cache.cumulative_hit_rate() * 100.0
        );
        assert!(
            cache.stats.verdict().starts_with("GOOD") || cache.stats.verdict().starts_with("OK"),
            "verdict={}",
            cache.stats.verdict()
        );
    }

    /// Mesh tier 要求で full_mesh を持たないエントリ (例: 遠方 Svo 格納からの
    /// 接近) もミスとして 1 回だけ計上される (旧実装は無計上のサイレント
    /// ミスで hit_rate を偽装していた — wave 18 回帰固定)。
    #[test]
    fn mesh_tier_miss_without_full_mesh_is_counted() {
        let mut cache = FrameReuseCache::new(true);
        let lod = LodHybridSelector::new(true, true, true);
        let (_sections, rle, _mesh, occupied) = build_entry(0, 0);
        cache.begin_frame(1);
        // full_mesh: None + encoding: Mesh (Svo 格納チャンクへの接近と同型)
        cache.store(0, 0, &rle, &occupied, None, None, ReuseEncoding::Mesh);
        let before = (cache.stats.misses, cache.stats.stale_invalidations);
        assert!(
            cache.try_reuse(0, 0, 32.0, &lod).is_none(),
            "no mesh => miss"
        );
        assert_eq!(
            cache.stats.misses,
            before.0 + 1,
            "miss must be counted once"
        );
        assert_eq!(
            cache.stats.stale_invalidations,
            before.1 + 1,
            "encoding mismatch family must count as stale"
        );
        assert_eq!(cache.cumulative_misses, 1);
        assert_eq!(cache.stats.hits, 0);
    }

    /// hits + misses == try_reuse 試行数の統計契約を決定的掃引で不変式化。
    /// (旧実装のサイレントミス or 呼出側二重計上はどちらもこれを破る)
    #[test]
    fn stats_contract_attempts_equal_hits_plus_misses() {
        let mut cache = FrameReuseCache::new(true);
        let lod = LodHybridSelector::new(true, true, true);
        let mut attempts = 0u64;
        let mut seed = 0xB5297A4D_u64;
        let mut next = move || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            seed >> 33
        };
        cache.begin_frame(1);
        for _ in 0..800 {
            let k = (next() % 6) as i32;
            if next() % 2 == 0 {
                let (_s, rle, mesh, occupied) = build_entry(k, 0);
                cache.store(k, 0, &rle, &occupied, Some(mesh), None, ReuseEncoding::Mesh);
                if next() % 4 == 0 {
                    // 全メッシュ無し格納を混ぜてサイレントミス経路も踏む
                    cache.store(k + 10, 0, &rle, &occupied, None, None, ReuseEncoding::Mesh);
                }
            } else {
                attempts += 1;
                let _ = cache.try_reuse(k + if next() % 3 == 0 { 10 } else { 0 }, 0, 32.0, &lod);
            }
        }
        let s = &cache.stats;
        assert_eq!(
            s.hits as u64 + s.misses as u64,
            attempts,
            "hits({}) + misses({}) must equal attempts({attempts})",
            s.hits,
            s.misses
        );
    }

    /// 同一 tick に大量格納 (last_tick タイ頻発) した容量超過で、追い出しが
    /// キー座標辞書順の厳密最小から決定的に行われる。
    /// 旧実装は最小タイを HashMap 反復順 (RandomState 由来 = プロセス毎非再現)
    /// に委ねていた。2 つの独立キャッシュ (独立 hasher) で完全一致も確認する。
    #[test]
    fn eviction_tiebreak_is_deterministic() {
        fn fill() -> FrameReuseCache {
            let mut cache = FrameReuseCache::new(true);
            cache.begin_frame(1);
            for cx in -3..21i32 {
                for cz in -2..20i32 {
                    // 24*22 = 528 エントリ (容量 512 を 16 超過)
                    cache.store(cx, cz, &[], &[], None, None, ReuseEncoding::None);
                }
            }
            cache
        }
        let a = fill();
        let b = fill();
        for cx in -3..21i32 {
            for cz in -2..20i32 {
                assert_eq!(
                    a.entries.contains_key(&(cx, cz)),
                    b.entries.contains_key(&(cx, cz)),
                    "survivor sets must match across independent caches: ({cx},{cz})"
                );
            }
        }
        // 最後に 512 生き残るのは辞書順で大きい側 512 件: 先頭 16 件
        // ((-3,-2)..=(-3,13)) が追い出され、(-3,14) 以降が残る。
        for cz in -2..14i32 {
            assert!(!a.entries.contains_key(&(-3, cz)), "evicted: (-3,{cz})");
        }
        assert!(a.entries.contains_key(&(-3, 14)));
        assert!(a.entries.contains_key(&(20, 19)));
        assert_eq!(a.entries.len(), 512);
        assert_eq!(a.stats.evictions, 16);
    }

    /// 鮮度追い出しの境界が厳密 STALE_FRAMES である (last_tick == stale_before は
    /// 生き残り、< は追い出しの off-by-one 固定)。
    #[test]
    fn stale_eviction_boundary_is_exact() {
        let mut cache = FrameReuseCache::new(true);
        cache.begin_frame(1);
        cache.store(0, 0, &[], &[], None, None, ReuseEncoding::None);
        // tick 121: stale_before = 1 → last_tick(1) < 1 は偽 → 生き残り
        cache.begin_frame(121);
        cache.end_frame();
        assert!(
            cache.entries.contains_key(&(0, 0)),
            "frame 121 must survive"
        );
        assert_eq!(cache.stats.evictions, 0);
        // tick 122: stale_before = 2 → 1 < 2 → 追い出し (ちょうど 120 フレーム無更新)
        cache.begin_frame(122);
        cache.end_frame();
        assert!(!cache.entries.contains_key(&(0, 0)), "frame 122 must evict");
        assert_eq!(cache.stats.evictions, 1);
    }

    /// memory_bytes が実保持量 (rle runs*4 + occupied*4 + verts*12 + indices*4
    /// + svo) のみを数える (旧実装は保持しない sections ~8KB/節を幽霊計上し
    /// occupied を無視していた — 厳密式で回帰固定)。
    #[test]
    fn entry_bytes_counts_only_real_storage() {
        use bytemuck::Zeroable;
        let rle = vec![
            RleSection {
                runs: vec![
                    RleRun {
                        block: 1,
                        count: 4096
                    };
                    3
                ],
            },
            RleSection {
                runs: vec![RleRun { block: 2, count: 1 }; 5],
            },
        ];
        let occupied = vec![0u32, 1];
        let mesh = BuiltChunkMesh {
            chunk_x: 0,
            chunk_z: 0,
            vertices: vec![Quantized12ByteVertex::zeroed(); 10],
            indices: vec![0u32; 30],
            is_empty: false,
        };
        let bytes = FrameReuseCache::entry_bytes(&rle, &occupied, Some(&mesh), None);
        assert_eq!(bytes, (3 + 5) * 4 + 2 * 4 + 10 * 12 + 30 * 4);
        // エントリ削除を伴う帳簿と store 上書きの整合:
        let mut cache = FrameReuseCache::new(true);
        cache.begin_frame(1);
        cache.store(
            0,
            0,
            &rle,
            &occupied,
            Some(mesh.clone()),
            None,
            ReuseEncoding::Mesh,
        );
        cache.refresh_memory_stat();
        assert_eq!(cache.stats.memory_bytes, bytes as u64);
        let small = BuiltChunkMesh {
            vertices: vec![],
            indices: vec![],
            is_empty: true,
            ..mesh
        };
        cache.store(
            0,
            0,
            &rle,
            &occupied,
            Some(small),
            None,
            ReuseEncoding::Mesh,
        );
        assert_eq!(
            cache.stats.memory_bytes,
            ((3 + 5) * 4 + 2 * 4) as u64,
            "overwrite must replace, not accumulate"
        );
        cache.invalidate(0, 0);
        assert_eq!(cache.stats.memory_bytes, 0);
        assert_eq!(cache.stats.stale_invalidations, 1);
    }
}
