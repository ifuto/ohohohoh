//! Per-chunk frame-to-frame reuse — skip prepare/RLE/mesh/SVO when terrain unchanged.
//!
//! Telemetry (`FrameReuseStats`) lets us see whether reuse helps (high hit rate, low
//! memory) or hurts (evictions, stale rebuilds, RAM bloat).

use crate::binary_greedy_meshing::SECTION_SIZE;
use crate::binary_greedy_meshing::SectionPalette;
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
    fingerprint: u64,
    sections: Vec<SectionPalette>,
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

    pub fn rle_fingerprint(rle: &[RleSection]) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for sec in rle {
            for run in &sec.runs {
                run.block.hash(&mut h);
                run.count.hash(&mut h);
            }
        }
        h.finish()
    }

    fn entry_bytes(
        sections: &[SectionPalette],
        rle: &[RleSection],
        mesh: Option<&BuiltChunkMesh>,
        svo: Option<&SparseVoxelOctree>,
    ) -> usize {
        let sec_bytes = sections.len() * SECTION_SIZE * SECTION_SIZE * SECTION_SIZE * 2;
        let rle_bytes: usize = rle.iter().map(|s| s.runs.len() * 4).sum();
        let mesh_bytes = mesh
            .map(|m| m.vertices.len() * 12 + m.indices.len() * 4)
            .unwrap_or(0);
        let svo_bytes = svo.map(|s| s.memory_bytes()).unwrap_or(0);
        sec_bytes + rle_bytes + mesh_bytes + svo_bytes
    }

    /// Try reuse without re-running prepare/mesh/SVO. Returns None on miss.
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

        let full = entry.full_mesh.as_ref()?;
        let mesh = if tier == LodTier::Near {
            full.clone()
        } else {
            self.stats.lod_re_simplify += 1;
            lod.simplify_mesh(full.clone(), tier)
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

    fn touch(&mut self, key: (i32, i32), _old_tick: u64) {
        if let Some(e) = self.entries.get_mut(&key) {
            e.last_tick = self.tick;
        }
    }

    pub fn record_miss(&mut self) {
        self.stats.misses += 1;
        self.cumulative_misses += 1;
    }

    pub fn store(
        &mut self,
        cx: i32,
        cz: i32,
        sections: &[SectionPalette],
        rle: &[RleSection],
        occupied: &[u32],
        full_mesh: Option<BuiltChunkMesh>,
        svo: Option<SparseVoxelOctree>,
        encoding: ReuseEncoding,
    ) {
        if !self.enabled {
            return;
        }
        let fingerprint = Self::rle_fingerprint(rle);
        let memory_bytes = Self::entry_bytes(sections, rle, full_mesh.as_ref(), svo.as_ref());
        let entry = ChunkFrameEntry {
            fingerprint,
            sections: sections.to_vec(),
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
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.last_tick)
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
    use crate::binary_greedy_meshing::{demo_column_palettes, mesh_chunk_column};
    use crate::section_rle::occupied_section_indices;
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
            cache.store(
                cx,
                cz,
                &sections,
                &rle,
                &occupied,
                None,
                Some(svo),
                ReuseEncoding::Svo,
            );
        } else {
            cache.store(
                cx,
                cz,
                &sections,
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
}
