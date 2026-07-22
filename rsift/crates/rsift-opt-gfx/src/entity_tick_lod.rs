//! Distance-banded entity tick scheduling (Tier 5).

use crate::spatial_hash::SpatialHashGrid3D;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickBand {
    EveryTick,
    Every2,
    Every4,
    Every8,
    RenderOnly,
}

#[derive(Debug)]
pub struct EntityTickScheduler {
    pub near_dist: f32,
    pub mid_dist: f32,
    pub far_dist: f32,
    pub cull_dist: f32,
    tick: u64,
}

impl Default for EntityTickScheduler {
    fn default() -> Self {
        Self {
            near_dist: 32.0,
            mid_dist: 64.0,
            far_dist: 128.0,
            cull_dist: 256.0,
            tick: 0,
        }
    }
}

impl EntityTickScheduler {
    pub fn advance(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    /// **契約 (2026-07-22 wave 34 明文化)**: `dist` は有限必須。
    /// 旧実装は NaN を全比較不成立で else に落とし **RenderOnly 扱いで
    /// 静寂に飢餓**させた (NaN は「遠い」のではなく観測欠損)。
    /// ±∞ (有限入力の overflow) も Minecraft スケール (±30M blocks) の
    /// 契約外として併せて拒否する。
    pub fn band_for_distance(&self, dist: f32) -> TickBand {
        assert!(
            dist.is_finite(),
            "band_for_distance 契約違反: dist は有限必須 (dist={dist})"
        );
        if dist < self.near_dist {
            TickBand::EveryTick
        } else if dist < self.mid_dist {
            TickBand::Every2
        } else if dist < self.far_dist {
            TickBand::Every4
        } else if dist < self.cull_dist {
            TickBand::Every8
        } else {
            TickBand::RenderOnly
        }
    }

    pub fn should_tick(&self, band: TickBand) -> bool {
        match band {
            TickBand::EveryTick => true,
            TickBand::Every2 => self.tick % 2 == 0,
            TickBand::Every4 => self.tick % 4 == 0,
            TickBand::Every8 => self.tick % 8 == 0,
            TickBand::RenderOnly => false,
        }
    }

    /// Returns entity ids that should AI-tick this frame.
    pub fn select_tickable(
        &self,
        positions: &[(u32, f32, f32, f32)],
        player: [f32; 3],
    ) -> Vec<u32> {
        let mut out = Vec::new();
        for &(id, x, y, z) in positions {
            let dx = x - player[0];
            let dy = y - player[1];
            let dz = z - player[2];
            let dist = (dx * dx + dy * dy + dz * dz).sqrt();
            let band = self.band_for_distance(dist);
            if self.should_tick(band) {
                out.push(id);
            }
        }
        out
    }

    /// `select_tickable` の空間ハッシュ前絞り版。
    ///
    /// **契約 (2026-07-22 wave 34 明文化)**: 返却は「hash ∩ positions ∩
    /// その tick の帯」。hash に存在しない id は距離が近くても**一切
    /// tick されない** — 呼び出し側は hash を常に positions ⊇ に保つ
    /// 責務を持つ (陳腐 hash での近距離飢餓を防ぐため)。
    /// positions にのみある id は無視、hash にのみある id も無視。
    pub fn select_tickable_hashed(
        &self,
        hash: &SpatialHashGrid3D,
        positions: &[(u32, f32, f32, f32)],
        player: [f32; 3],
    ) -> Vec<u32> {
        let candidates = hash.query_radius(player[0], player[1], player[2], self.cull_dist);
        let mut pos_map: std::collections::HashMap<u32, [f32; 3]> = positions
            .iter()
            .map(|&(id, x, y, z)| (id, [x, y, z]))
            .collect();
        let mut out = Vec::new();
        for id in candidates {
            if let Some(p) = pos_map.remove(&id) {
                let dx = p[0] - player[0];
                let dy = p[1] - player[1];
                let dz = p[2] - player[2];
                let dist = (dx * dx + dy * dy + dz * dz).sqrt();
                if self.should_tick(self.band_for_distance(dist)) {
                    out.push(id);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn far_entity_skips() {
        let mut s = EntityTickScheduler::default();
        s.tick = 1; // odd → Every2 skips
        let ids = s.select_tickable(&[(1, 50.0, 0.0, 0.0)], [0.0, 0.0, 0.0]);
        assert!(ids.is_empty());
        s.tick = 2;
        let ids = s.select_tickable(&[(1, 50.0, 0.0, 0.0)], [0.0, 0.0, 0.0]);
        assert_eq!(ids, vec![1]);
    }

    /// wave 34-1: 帯境界の厳密列 (等号は次帯へ落ちる)。
    #[test]
    fn band_boundaries_exact() {
        let s = EntityTickScheduler::default();
        let cases: &[(f32, TickBand)] = &[
            (0.0, TickBand::EveryTick),
            (31.5, TickBand::EveryTick),
            (32.0, TickBand::Every2), // 等号 → 次帯
            (63.0, TickBand::Every2),
            (64.0, TickBand::Every4),
            (128.0, TickBand::Every8),
            (256.0, TickBand::RenderOnly), // cull 等号 → RenderOnly
            (1000.0, TickBand::RenderOnly),
        ];
        for (dist, want) in cases {
            assert_eq!(s.band_for_distance(*dist), *want, "dist={dist}");
        }
    }

    /// wave 34-2: 各帯の tick 位相を厳密列でピン (t=0..9) + advance の
    /// wrap 跨ぎで mod 2^k が破れないこと。
    #[test]
    fn should_tick_phase_sequences_and_wraparound() {
        let mut s = EntityTickScheduler::default();
        // Every2/4/8 の t=0..9 厳密列 (advance で位相前進)
        fn seq_of(band: TickBand) -> Vec<bool> {
            let mut s = EntityTickScheduler::default();
            (0..10)
                .map(|_| {
                    let r = s.should_tick(band);
                    s.advance();
                    r
                })
                .collect()
        }
        let seq2 = seq_of(TickBand::Every2);
        assert_eq!(
            seq2,
            [true, false, true, false, true, false, true, false, true, false]
        );
        let seq4 = seq_of(TickBand::Every4);
        assert_eq!(
            seq4,
            [true, false, false, false, true, false, false, false, true, false]
        );
        let seq8 = seq_of(TickBand::Every8);
        assert_eq!(
            seq8,
            [true, false, false, false, false, false, false, false, true, false]
        );
        // RenderOnly は常に不 tick、EveryTick は常に tick
        assert!(!s.should_tick(TickBand::RenderOnly));
        assert!(s.should_tick(TickBand::EveryTick));
        // wraparound: MAX からの advance で 0 に戻り、以後の位相は連続的に破れない
        s.tick = u64::MAX - 1;
        s.advance(); // MAX
        assert!(!s.should_tick(TickBand::Every2), "MAX は奇数");
        s.advance(); // 0 (wrap)
        assert!(
            s.should_tick(TickBand::Every2),
            "0 は偶数 — 2^k 帯は wrap で破れない"
        );
    }

    /// wave 34-3: select_tickable の厳密集合・順序 (入力順維持) と
    /// exact 距離 (3-4-5 三角形)。
    #[test]
    fn select_tickable_exact_membership_and_order() {
        let mut s = EntityTickScheduler::default();
        s.tick = 4; // 偶数 (Every2/Every4 は tick、Every8 は skip: 4%8=4)
        let positions = [
            (7, 3.0, 4.0, 0.0),   // dist 5 → EveryTick ✓
            (3, 0.0, 0.0, 96.0),  // dist 96 → Every4、t=4 → tick ✓
            (9, 40.0, 0.0, 0.0),  // dist 40 → Every2 ✓
            (1, 200.0, 0.0, 0.0), // dist 200 → Every8、t=4 → skip
            (5, 0.0, 0.0, 300.0), // dist 300 → RenderOnly skip
        ];
        let ids = s.select_tickable(&positions, [0.0, 0.0, 0.0]);
        assert_eq!(ids, vec![7, 3, 9], "帯×位相厳密、順序は入力順");
    }

    /// wave 34-4: hashed 経路は hash ∩ positions ∩ 帯。id 昇順入力では
    /// (hash query のソート順と一致して) naive 版と逐一致。
    #[test]
    fn select_tickable_hashed_matches_naive_on_sorted_ids() {
        let mut s = EntityTickScheduler::default();
        s.tick = 8; // Every2/4/8 全て tick 位相
        let positions = [
            (1, 10.0, 0.0, 0.0),  // EveryTick
            (2, 40.0, 0.0, 0.0),  // Every2
            (3, 100.0, 0.0, 0.0), // Every4
            (4, 200.0, 0.0, 0.0), // Every8
            (5, 500.0, 0.0, 0.0), // RenderOnly (cull 外: hash query にも出ない)
        ];
        let mut hash = crate::spatial_hash::SpatialHashGrid3D::new(16.0);
        for &(id, x, y, z) in &positions {
            hash.insert(id, x, y, z);
        }
        let naive = s.select_tickable(&positions, [0.0, 0.0, 0.0]);
        let hashed = s.select_tickable_hashed(&hash, &positions, [0.0, 0.0, 0.0]);
        assert_eq!(naive, vec![1, 2, 3, 4]);
        assert_eq!(hashed, naive, "id 昇順・hash 完備では逐一致");
        // hash から id=1 (近距離) を消す → naive には残るが hashed には出ない
        hash.remove(1);
        let hashed2 = s.select_tickable_hashed(&hash, &positions, [0.0, 0.0, 0.0]);
        assert_eq!(
            hashed2,
            vec![2, 3, 4],
            "hash 陳腐 id は近くても tick されない (契約)"
        );
        // positions にだけある id は無視される
        let extra = [(6, 5.0, 0.0, 0.0)];
        let hashed3 = s.select_tickable_hashed(&hash, &extra, [0.0, 0.0, 0.0]);
        assert!(hashed3.is_empty(), "positions のみの id は tick されない");
    }

    /// wave 34-5: 非有限座標は fail-loud (NaN / inf の両方)。
    #[test]
    #[should_panic(expected = "band_for_distance 契約違反")]
    fn nan_position_rejected() {
        let s = EntityTickScheduler::default();
        let _ = s.select_tickable(&[(1, f32::NAN, 0.0, 0.0)], [0.0, 0.0, 0.0]);
    }

    #[test]
    #[should_panic(expected = "band_for_distance 契約違反")]
    fn infinite_position_rejected() {
        let s = EntityTickScheduler::default();
        let _ = s.select_tickable(&[(1, f32::INFINITY, 0.0, 0.0)], [0.0, 0.0, 0.0]);
    }
}
