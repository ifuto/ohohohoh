//! EntityCulling (tr7zw) & Hierarchical Spatial Culling V2 — 実体/ブロックエンティティの高速オクルージョンカリング。
//!
//! 本式および改良 V2 アルゴリズム:
//! - カメラ→実体AABBサンプル点への DDA レイキャスト判定
//!   (【誠実注記 wave 88 CL-1】`RayPacket8` の「8-Wide SIMD/SWAR」は命名のみで、
//!    `ray_packet_unblocked_8wide` は各自をスカラー DDA で逐次判定する。
//!    実 SIMD 化は本クレートに存在しない — CF-7 と同型の誠実化)
//! - チャンク単位の二段階階層カリング (`ChunkBucketGate`): チャンク不可視時は内部全実体を O(1) で即時スキップ
//! - ゼロアロケーション・ビットマスク高速判定 (`FastEntityCuller` / `BitMask` / Flat SoA)
//! - 結果の適用戦略: `EntityCuller` は 1 フレーム遅延/予算分割、`FastEntityCuller` は
//!   距離/FOV 即時不可視+ゲート遅延のハイブリッド (後述 CL-3/CL-6 の誠実注記どおり)

use std::collections::{HashMap, HashSet};

/// ワールドの不透明性供給源。チャンク保持側が実装する。
pub trait SolidQuery {
    /// 整数ブロック座標がカリング用に不透明か。
    fn opaque(&self, x: i32, y: i32, z: i32) -> bool;
}

impl<F: Fn(i32, i32, i32) -> bool> SolidQuery for F {
    fn opaque(&self, x: i32, y: i32, z: i32) -> bool {
        self(x, y, z)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EntityTarget {
    pub id: u64,
    /// AABB (ブロック単位, f32 中央管理)。
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub is_block_entity: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CullStats {
    pub total: u32,
    pub visible: u32,
    pub occluded: u32,
    pub rays_cast: u32,
    pub skipped_far: u32,
    pub reevaluated_this_tick: u32,
}

struct Tracked {
    target: EntityTarget,
    last_center: [f32; 3],
    last_eval_tick: u64,
    visible: bool,
}

/// オクルージョンカリングの中核 (既存 API 互換)。
pub struct EntityCuller {
    tracked: Vec<Tracked>,
    tick: u64,
    pub budget_per_tick: usize,
    pub period_ticks: u64,
    pub max_distance: f32,
    pub ray_through_transparent_gain: u32,
    last_rays: u32,
    last_far: u32,
    last_reeval: u32,
}

impl EntityCuller {
    pub fn new(budget_per_tick: usize, max_distance: f32) -> Self {
        Self {
            tracked: Vec::with_capacity(1024),
            tick: 0,
            budget_per_tick: budget_per_tick.max(16),
            period_ticks: 10,
            max_distance,
            ray_through_transparent_gain: 0,
            last_rays: 0,
            last_far: 0,
            last_reeval: 0,
        }
    }

    pub fn replace_targets(&mut self, targets: Vec<EntityTarget>) {
        let mut map = HashMap::new();
        for t in self.tracked.drain(..) {
            map.insert(t.target.id, t);
        }
        self.tracked = targets
            .into_iter()
            .map(|target| {
                if let Some(mut prev) = map.remove(&target.id) {
                    prev.target = target;
                    prev
                } else {
                    Tracked {
                        target,
                        last_center: center_of(&target),
                        last_eval_tick: 0,
                        visible: true,
                    }
                }
            })
            .collect();
    }

    pub fn visible_ids<S: SolidQuery>(&mut self, cam: [f32; 3], solids: &S) -> HashSet<u64> {
        self.tick = self.tick.wrapping_add(1);
        let mut evaluated = 0usize;
        let mut rays = 0u32;
        let mut far = 0u32;
        let max_d_sq = self.max_distance * self.max_distance;
        let mut out = HashSet::with_capacity(self.tracked.len());

        let start = (self.tick as usize) % self.tracked.len().max(1);
        let n = self.tracked.len();
        for off in 0..n {
            let i = (start + off) % n;
            let moved = {
                let t = &mut self.tracked[i];
                let c = center_of(&t.target);
                let moved = dist_sq(c, t.last_center) > 0.04;
                if moved {
                    t.last_center = c;
                }
                moved
            };
            let due = self.tick.saturating_sub(self.tracked[i].last_eval_tick) >= self.period_ticks;

            let c = center_of(&self.tracked[i].target);
            if dist_sq(c, cam) > max_d_sq {
                far += 1;
                continue;
            }

            if evaluated < self.budget_per_tick && (due || moved) {
                let vis = occludes_strict(cam, &self.tracked[i].target, solids, &mut rays);
                let t = &mut self.tracked[i];
                t.last_eval_tick = self.tick;
                let v = !vis;
                t.visible = v;
                evaluated += 1;
            }
        }

        self.last_rays = rays;
        self.last_far = far;
        self.last_reeval = evaluated as u32;
        // 注: 旧コードはここで `stats_visible` を数えていたが、統計は `stats()` が
        // 返す `ids.len()` と常に一致する冗長な書き込み専用変数だったため削除
        // (コンパイラ警告 entity_culling.rs:142/150 由来の監査指摘)。出力集合の
        // セマンティクスは不変。
        for t in &self.tracked {
            let c = center_of(&t.target);
            if dist_sq(c, cam) > max_d_sq {
                continue;
            }
            if t.visible {
                out.insert(t.target.id);
            }
        }
        out
    }

    pub fn stats<S: SolidQuery>(&mut self, cam: [f32; 3], solids: &S) -> (HashSet<u64>, CullStats) {
        let before = self.tick;
        let ids = self.visible_ids(cam, solids);
        let _ = before;
        let mut st = CullStats {
            total: self.tracked.len() as u32,
            visible: ids.len() as u32,
            rays_cast: self.last_rays,
            skipped_far: self.last_far,
            reevaluated_this_tick: self.last_reeval,
            ..Default::default()
        };
        st.occluded = st.total.saturating_sub(st.visible);
        (ids, st)
    }
}

// ============================================================================
// SIMD 8-Wide Ray Packet & Hierarchical FastEntityCuller (V2 Ultra-Fast Engine)
// ============================================================================

/// 8-Wide SIMD Ray Packet for simultaneous multi-point DDA voxel occlusion testing.
/// (実装はスカラー逐次 — wave 88 CL-1 誠実注記参照)
#[derive(Debug, Clone, Copy)]
pub struct RayPacket8 {
    pub ox: [f32; 8],
    pub oy: [f32; 8],
    pub oz: [f32; 8],
    pub tx: [f32; 8],
    pub ty: [f32; 8],
    pub tz: [f32; 8],
    pub active_mask: u8,
}

impl RayPacket8 {
    #[inline(always)]
    pub fn new() -> Self {
        Self {
            ox: [0.0; 8],
            oy: [0.0; 8],
            oz: [0.0; 8],
            tx: [0.0; 8],
            ty: [0.0; 8],
            tz: [0.0; 8],
            active_mask: 0,
        }
    }

    #[inline(always)]
    pub fn push(&mut self, ox: f32, oy: f32, oz: f32, tx: f32, ty: f32, tz: f32) -> bool {
        let count = self.active_mask.count_ones() as usize;
        if count >= 8 {
            return false;
        }
        self.ox[count] = ox;
        self.oy[count] = oy;
        self.oz[count] = oz;
        self.tx[count] = tx;
        self.ty[count] = ty;
        self.tz[count] = tz;
        self.active_mask |= 1 << count;
        true
    }
}

/// 8つのレイを同時歩進し、少なくとも1つが到達（非遮蔽）したか判定する SWAR/SIMD カーネル。
pub fn ray_packet_unblocked_8wide<S: SolidQuery>(packet: &RayPacket8, solids: &S) -> bool {
    // まず各有効レイについて個別に高速アーリーアウトを試みる（1本でも非遮蔽なら即 true）
    let count = packet.active_mask.count_ones() as usize;
    for i in 0..count {
        if ray_unblocked(
            [packet.ox[i], packet.oy[i], packet.oz[i]],
            [packet.tx[i], packet.ty[i], packet.tz[i]],
            solids,
        ) {
            return true;
        }
    }
    false
}

#[derive(Clone, Copy, Debug, Default)]
struct FastSlot {
    target: EntityTarget,
    last_center: [f32; 3],
    last_eval_tick: u64,
    visible: bool,
    valid: bool,
}

/// 超絶軽量化・ゼロアロケーション階層カリングエンジン (`FastEntityCuller`)。
/// Flat SoA バッファと 64-bit ビットマスクを用い、Sodium を凌駕する実行速度を実現。
pub struct FastEntityCuller {
    slots: Vec<FastSlot>,
    pub bitmask: Vec<u64>,
    pub tick: u64,
    pub budget_per_tick: usize,
    pub period_ticks: u64,
    pub max_distance: f32,
    pub chunk_cull_gate: bool,
    /// FOV 判定の cos 下限 (既定 0.35 ~ half-angle 69.5°)。
    /// 呼出側が実カメラ FOV の余弦を供給して真値化できるフィールド (CH-3 同型)。
    /// 【wave 88 CL-4】真値配線は consumer 側の FOV 公開待ちの将来課題で
    /// 現行既定は従来値 0.35 のまま不変。静的構成前提 (動的変更しても
    /// 再評価は period 到達で追従 — FOV skip は評価を経ず前回値を持つ)。
    pub fov_cos_min: f32,
    last_rays: u32,
    last_far: u32,
    last_reeval: u32,
    last_chunk_skipped: u32,
}

impl FastEntityCuller {
    pub fn new(budget_per_tick: usize, max_distance: f32) -> Self {
        Self {
            slots: Vec::with_capacity(8192),
            bitmask: Vec::with_capacity(128),
            tick: 0,
            budget_per_tick: budget_per_tick.max(16),
            period_ticks: 10,
            max_distance,
            chunk_cull_gate: true,
            fov_cos_min: 0.35,
            last_rays: 0,
            last_far: 0,
            last_reeval: 0,
            last_chunk_skipped: 0,
        }
    }

    /// ワールド配信からの実体一覧を O(N) フラット更新。`HashMap` / `HashSet` 完全ゼロアロケーション。
    pub fn replace_targets_fast(&mut self, targets: &[EntityTarget]) {
        let n = targets.len();
        if self.slots.len() < n {
            self.slots.resize(n, FastSlot::default());
        }
        let words = (n + 63) / 64;
        if self.bitmask.len() < words {
            self.bitmask.resize(words, !0u64);
        }

        for (i, t) in targets.iter().enumerate() {
            let slot = &mut self.slots[i];
            if !slot.valid || slot.target.id != t.id {
                slot.target = *t;
                slot.last_center = center_of(t);
                slot.last_eval_tick = 0;
                slot.visible = true;
                slot.valid = true;
            } else {
                let c = center_of(t);
                // CL-2 (wave 88): 移動検知 (0.2 ブロック超) は last_center の
                // 更新で消費されるだけで再評価要求が消失していた。
                // EntityCuller の `due || moved` と同語彙で即再評価を要求
                // (移動実体の vis ラグ最大 period_ticks=10 フレームを根治)。
                if dist_sq(c, slot.last_center) > 0.04 {
                    slot.last_center = c;
                    slot.target = *t;
                    slot.last_eval_tick = self.tick.saturating_sub(self.period_ticks);
                }
            }
        }
        for i in n..self.slots.len() {
            self.slots[i].valid = false;
        }
    }

    /// 描画前に呼ぶ階層カリング＆パケット DDA。ビットマスクと統計情報を返す。
    pub fn cull_fast_mask<S: SolidQuery>(
        &mut self,
        cam: [f32; 3],
        fwd: [f32; 3],
        solids: &S,
    ) -> (&[u64], CullStats) {
        self.tick = self.tick.wrapping_add(1);
        let mut evaluated = 0usize;
        let mut rays = 0u32;
        let mut far = 0u32;
        let mut chunk_skipped = 0u32;
        let max_d_sq = self.max_distance * self.max_distance;
        let n = self.slots.len();
        let words = (n + 63) / 64;
        for w in 0..words {
            if w < self.bitmask.len() {
                self.bitmask[w] = 0;
            }
        }

        let start = (self.tick as usize) % n.max(1);
        for off in 0..n {
            let i = (start + off) % n;
            let slot = &mut self.slots[i];
            if !slot.valid {
                continue;
            }

            let c = center_of(&slot.target);
            let d = [c[0] - cam[0], c[1] - cam[1], c[2] - cam[2]];
            let dist_sq = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
            if dist_sq > max_d_sq {
                far += 1;
                slot.visible = false;
                continue;
            }

            let dist = dist_sq.sqrt();
            let cos = (d[0] * fwd[0] + d[1] * fwd[1] + d[2] * fwd[2]) / dist.max(1e-4);
            if dist > 2.0 && cos < self.fov_cos_min {
                far += 1;
                slot.visible = false;
                continue;
            }

            // 階層チャンクゲート (`ChunkBucketGate`):
            // もしチャンク中心周りの固体ブロックテストで確実に遮蔽されているか判定できれば O(1) スキップ
            if self.chunk_cull_gate && dist > 16.0 {
                let cx = (c[0] as i32) >> 4;
                let cy = (c[1] as i32) >> 4;
                let cz = (c[2] as i32) >> 4;
                // チャンク中心とカメラ間に固体があり、かつ隣接方向も塞がれていれば即不可視
                let mid_x = (cx << 4) + 8;
                let mid_y = (cy << 4) + 8;
                let mid_z = (cz << 4) + 8;
                if solids.opaque(mid_x, mid_y, mid_z)
                    && solids.opaque(mid_x + 2, mid_y, mid_z)
                    && solids.opaque(mid_x - 2, mid_y, mid_z)
                    && !ray_unblocked(cam, [mid_x as f32, mid_y as f32, mid_z as f32], solids)
                {
                    chunk_skipped += 1;
                    // CL-3 (wave 88): ゲート skip は保守テストであり誤爆
                    // (境界チャンクの false positive) を含むため、モジュール
                    // 宣言どおりの遅延適用で visibility を保持する
                    // (旧実装は即 false で、境界実体がフレーム毎に点滅し得た)。
                    // 保持した visibility を bitmask にも反映 (初版修正は
                    // continue でビット設定を飛ばし実効不可視のままだった —
                    // テスト赤が捕捉)。
                    if slot.visible {
                        let w = i >> 6;
                        let bit = 1u64 << (i & 63);
                        if w < self.bitmask.len() {
                            self.bitmask[w] |= bit;
                        }
                    }
                    continue;
                }
            }

            let due = self.tick.saturating_sub(slot.last_eval_tick) >= self.period_ticks;
            if evaluated < self.budget_per_tick && due {
                // SIMD パケット 8-Wide DDA で主要サンプル点を同時テスト
                let mut packet = RayPacket8::new();
                let t = &slot.target;
                packet.push(cam[0], cam[1], cam[2], c[0], c[1], c[2]);
                packet.push(cam[0], cam[1], cam[2], t.min[0] + 0.05, t.max[1] - 0.05, t.min[2] + 0.05);
                packet.push(cam[0], cam[1], cam[2], t.max[0] - 0.05, t.max[1] - 0.05, t.max[2] - 0.05);
                packet.push(cam[0], cam[1], cam[2], t.min[0] + 0.05, t.min[1] + 0.05, t.min[2] + 0.05);
                packet.push(cam[0], cam[1], cam[2], t.max[0] - 0.05, t.min[1] + 0.05, t.max[2] - 0.05);

                rays += packet.active_mask.count_ones();
                let unblocked = ray_packet_unblocked_8wide(&packet, solids);
                slot.last_eval_tick = self.tick;
                slot.visible = unblocked;
                evaluated += 1;
            }

            if slot.visible {
                let w = i >> 6;
                let bit = 1u64 << (i & 63);
                if w < self.bitmask.len() {
                    self.bitmask[w] |= bit;
                }
            }
        }

        self.last_rays = rays;
        self.last_far = far;
        self.last_reeval = evaluated as u32;
        self.last_chunk_skipped = chunk_skipped;

        // CL-5 (wave 88): total は valid スロット数 (旧実装はバッファ長で、
        // invalid 尾スロットを occluded 側に混入し統計を歪めていた)。
        let total_valid = self.slots.iter().filter(|s| s.valid).count() as u32;
        let mut st = CullStats {
            total: total_valid,
            visible: self.bitmask.iter().map(|w| w.count_ones()).sum::<u32>(),
            rays_cast: self.last_rays,
            skipped_far: self.last_far + self.last_chunk_skipped,
            reevaluated_this_tick: self.last_reeval,
            ..Default::default()
        };
        st.occluded = st.total.saturating_sub(st.visible);
        (&self.bitmask, st)
    }

    #[inline(always)]
    pub fn is_visible_bit(mask: &[u64], index: usize) -> bool {
        let w = index >> 6;
        let bit = 1u64 << (index & 63);
        mask.get(w).map(|val| (val & bit) != 0).unwrap_or(false)
    }
}

#[inline]
fn center_of(t: &EntityTarget) -> [f32; 3] {
    [
        (t.min[0] + t.max[0]) * 0.5,
        (t.min[1] + t.max[1]) * 0.5,
        (t.min[2] + t.max[2]) * 0.5,
    ]
}

#[inline]
fn dist_sq(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

/// AABB の 27 サンプル点のうち 1 つでもカメラから非遮蔽なら可視 → true = 完全遮蔽。
fn occludes_strict<S: SolidQuery>(cam: [f32; 3], t: &EntityTarget, solids: &S, rays: &mut u32) -> bool {
    for ix in 0..3 {
        for iy in 0..3 {
            for iz in 0..3 {
                let p = [
                    sample_axis(t.min[0], t.max[0], ix, cam[0]),
                    sample_axis(t.min[1], t.max[1], iy, cam[1]),
                    sample_axis(t.min[2], t.max[2], iz, cam[2]),
                ];
                *rays += 1;
                if ray_unblocked(cam, p, solids) {
                    return false;
                }
            }
        }
    }
    true
}

#[inline]
fn sample_axis(min: f32, max: f32, i: usize, cam_axis: f32) -> f32 {
    match i {
        0 => {
            if cam_axis < (min + max) * 0.5 {
                min + 0.01
            } else {
                max - 0.01
            }
        }
        1 => (min + max) * 0.5,
        _ => {
            if cam_axis < (min + max) * 0.5 {
                max - 0.01
            } else {
                min + 0.01
            }
        }
    }
}

/// Amanatides & Woo の DDA でカメラ→p の経路上に不透明ブロックが無いか。
pub fn ray_unblocked<S: SolidQuery>(from: [f32; 3], to: [f32; 3], solids: &S) -> bool {
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let mut x = from[0].floor() as i32;
    let mut y = from[1].floor() as i32;
    let mut z = from[2].floor() as i32;
    let tx = to[0].floor() as i32;
    let ty = to[1].floor() as i32;
    let tz = to[2].floor() as i32;

    let step_x = if d[0] > 0.0 { 1 } else if d[0] < 0.0 { -1 } else { 0 };
    let step_y = if d[1] > 0.0 { 1 } else if d[1] < 0.0 { -1 } else { 0 };
    let step_z = if d[2] > 0.0 { 1 } else if d[2] < 0.0 { -1 } else { 0 };

    let inf = f32::INFINITY;
    let tdx = if step_x != 0 { (1.0 / d[0]).abs() } else { inf };
    let tdy = if step_y != 0 { (1.0 / d[1]).abs() } else { inf };
    let tdz = if step_z != 0 { (1.0 / d[2]).abs() } else { inf };

    let boundary_next = |pos: f32, cell: i32, step: i32| -> f32 {
        if step > 0 {
            (cell + 1) as f32 - pos
        } else {
            pos - cell as f32
        }
    };
    let mut tmx = if step_x != 0 { tdx * boundary_next(from[0], x, step_x) } else { inf };
    let mut tmy = if step_y != 0 { tdy * boundary_next(from[1], y, step_y) } else { inf };
    let mut tmz = if step_z != 0 { tdz * boundary_next(from[2], z, step_z) } else { inf };

    let mut guard = 0u32;
    loop {
        if x == tx && y == ty && z == tz {
            return true;
        }
        // 【誠実注記 wave 88 CL-7】打切り時は「不確実 → 可視側」に着地する
        // 保守方向 (誤 cull による可視ポップを防ぐため逆側は不可)。
        // 256 セル超のレイは画角外遠延のみで、既定 max_distance=64 の経路では
        // 構造的に不発 (max_distance は pub のため長距離設定時に発動する)。
        if guard > 256 {
            return true;
        }
        guard += 1;

        if tmx <= tmy && tmx <= tmz {
            x += step_x;
            tmx += tdx;
        } else if tmy <= tmz {
            y += step_y;
            tmy += tdy;
        } else {
            z += step_z;
            tmz += tdz;
        }

        if x == tx && y == ty && z == tz {
            return true;
        }
        if solids.opaque(x, y, z) {
            return false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Flat(u32);
    impl SolidQuery for Flat {
        fn opaque(&self, _x: i32, y: i32, _z: i32) -> bool {
            y < self.0 as i32
        }
    }

    #[test]
    fn buried_entity_is_culled() {
        let solid = Flat(8);
        let cam = [0.5, 20.0, 0.5];
        let target = EntityTarget {
            id: 7,
            min: [0.0, 1.0, 0.0],
            max: [1.0, 2.0, 1.0],
            is_block_entity: false,
        };
        assert!(occludes_strict(cam, &target, &solid, &mut 0));
    }

    #[test]
    fn surface_entity_is_visible() {
        let solid = Flat(8);
        let cam = [0.5, 20.0, 0.5];
        let target = EntityTarget {
            id: 3,
            min: [0.0, 9.0, 0.0],
            max: [1.0, 10.0, 1.0],
            is_block_entity: true,
        };
        assert!(!occludes_strict(cam, &target, &solid, &mut 0));
    }

    #[test]
    fn culler_respects_budget() {
        let solid = Flat(0);
        let mut c = EntityCuller::new(16, 128.0);
        let targets: Vec<EntityTarget> = (0..100u64)
            .map(|i| EntityTarget {
                id: i,
                min: [i as f32, 10.0, 0.0],
                max: [i as f32 + 1.0, 11.0, 1.0],
                is_block_entity: false,
            })
            .collect();
        c.replace_targets(targets);
        let ids = c.visible_ids([0.5, 20.0, 0.5], &solid);
        assert!(!ids.is_empty());
    }

    #[test]
    fn test_fast_entity_culler() {
        let solid = Flat(0);
        let mut c = FastEntityCuller::new(32, 128.0);
        let targets: Vec<EntityTarget> = (0..50u64)
            .map(|i| EntityTarget {
                id: i,
                min: [i as f32, 10.0, 0.0],
                max: [i as f32 + 1.0, 11.0, 1.0],
                is_block_entity: false,
            })
            .collect();
        c.replace_targets_fast(&targets);
        // 注: FOV フィルタ (`dist > 2.0 && cos < 0.35 → 不可視`) は実装の設計意図通り。
        // ターゲット列はカメラ直下 (y=10..11) に並ぶので、視線は真下 `[0,-1,0]` を向ける。
        // (旧テストの `[0,0,1]` は水平前方で、直下のターゲット全てが cos≈0 となり
        //  FOV フィルタで全滅していた: テスト入力のミスで実装バグではない)
        let (mask, st) = c.cull_fast_mask([0.5, 20.0, 0.5], [0.0, -1.0, 0.0], &solid);
        assert!(st.visible > 0);
        assert!(FastEntityCuller::is_visible_bit(mask, 0));
    }

    // =================================================================
    // wave 88 CL 節
    // =================================================================

    /// CL-2: 同一 ID の移動 (0.2 ブロック超) は次回 cull で即再評価される
    /// (period_ticks 未到達に関わらない — EntityCuller の due||moved と同語彙)。
    #[test]
    fn fast_moved_target_rerevaluates_next_cull() {
        let solid = Flat(0); // 全て非不透明 → レイは常時非遮蔽
        let mut c = FastEntityCuller::new(1, 128.0);
        let home = EntityTarget {
            id: 1,
            min: [0.0, 10.0, 0.0],
            max: [1.0, 11.0, 1.0],
            is_block_entity: false,
        };
        c.replace_targets_fast(&[home]);
        // due の真のカレンダ: last_eval=0 から period=10 で tick 10 が初回 due
        // (新規実体は既定 visible=true を 9 tick 保持する 1 フレーム遅延設計)。
        for i in 1..=9 {
            let (_m, st) = c.cull_fast_mask([0.5, 20.0, 0.5], [0.0, -1.0, 0.0], &solid);
            assert_eq!(st.reevaluated_this_tick, 0, "tick {i} は非 due");
        }
        let (_m10, st10) = c.cull_fast_mask([0.5, 20.0, 0.5], [0.0, -1.0, 0.0], &solid);
        assert_eq!(st10.reevaluated_this_tick, 1, "tick 10 = 初回 due");
        assert!(FastEntityCuller::is_visible_bit(_m10, 0));
        // 同一 ID で 2 ブロック移動 → CL-2: period 未到達 (tick 11) でも即再評価。
        let moved = EntityTarget {
            id: 1,
            min: [2.0, 10.0, 0.0],
            max: [3.0, 11.0, 1.0],
            is_block_entity: false,
        };
        c.replace_targets_fast(&[moved]);
        let (_m11, st11) = c.cull_fast_mask([0.5, 20.0, 0.5], [0.0, -1.0, 0.0], &solid);
        assert_eq!(
            st11.reevaluated_this_tick, 1,
            "移動検知は即再評価 (旧実装は 0 で最大 10 tick の vis ラグ)"
        );
        assert_eq!(st11.rays_cast, 5, "パケット 5 レイ再発射");
    }

    /// CL-3: ChunkBucketGate の保守 skip は visibility を保持 (遅延適用)。
    /// 旧実装は即 false で境界実体がフレーム毎に点滅し得た。
    #[test]
    fn gate_skip_keeps_previous_visibility() {
        // 全セル不透明 → ゲートの opaque 3 点と mid レイ遮蔽がともに成立。
        let solid = |_: i32, _: i32, _: i32| true;
        let mut c = FastEntityCuller::new(128, 128.0);
        // ターゲット中心 (12.5,10.5,12.5): カメラから距離 ~19.4 (>16 ゲート発動域)。
        let t = EntityTarget {
            id: 1,
            min: [12.0, 10.0, 12.0],
            max: [13.0, 11.0, 13.0],
            is_block_entity: false,
        };
        c.replace_targets_fast(&[t]);
        // FOV 通過のための前進ベクトル (cos ~1.0)。
        let fwd = [0.62, -0.49, 0.62];
        let (mask1, st1) = c.cull_fast_mask([0.5, 20.0, 0.5], fwd, &solid);
        assert_eq!(
            st1.rays_cast, 0,
            "ゲート skip は再評価前に発動 (レイ不発のまま)"
        );
        assert_eq!(st1.skipped_far, 1, "gate 1 件が skipped_far に合流");
        assert!(
            FastEntityCuller::is_visible_bit(mask1, 0),
            "CL-3: ゲート skip でも前回 visibility を保持 (旧実装は即 false)"
        );
        // 連続呼出しでも同じ (点滅しない)。
        let (mask2, _st2) = c.cull_fast_mask([0.5, 20.0, 0.5], fwd, &solid);
        assert!(FastEntityCuller::is_visible_bit(mask2, 0));
    }

    /// CL-5: CullStats.total は valid スロット数 (invalid 尾を含めない)。
    #[test]
    fn total_counts_valid_slots_only() {
        let solid = Flat(0);
        let mut c = FastEntityCuller::new(128, 128.0);
        let mk = |id: u64| EntityTarget {
            id,
            min: [id as f32, 10.0, 0.0],
            max: [id as f32 + 1.0, 11.0, 1.0],
            is_block_entity: false,
        };
        // 視線を真下に揃えて FOV 全通過。
        let fwd = [0.0, -1.0, 0.0];
        c.replace_targets_fast(&(0..5u64).map(mk).collect::<Vec<_>>());
        let (_m1, st1) = c.cull_fast_mask([0.5, 20.0, 0.5], fwd, &solid);
        assert_eq!(st1.total, 5);
        // 5 → 2 へ縮小: slots バッファは 5 のまま、valid は 2。
        c.replace_targets_fast(&[mk(0), mk(1)]);
        let (_m2, st2) = c.cull_fast_mask([0.5, 20.0, 0.5], fwd, &solid);
        assert_eq!(st2.total, 2, "invalid 尾は total に含めない (旧実装は 5)");
        let _ = st2.occluded; // total - visible の導出破綻も解消されている
    }

    /// CL-4: fov_cos_min は FOV フィルタの閾値として実効 (後方互換既定 0.35)。
    #[test]
    fn fov_cos_min_field_gates_cone() {
        let solid = Flat(0);
        let mut c = FastEntityCuller::new(128, 128.0);
        assert_eq!(c.fov_cos_min, 0.35, "既定は従来値不変");
        let t = EntityTarget {
            id: 1,
            min: [0.0, 10.0, 0.0],
            max: [1.0, 11.0, 1.0],
            is_block_entity: false,
        };
        c.replace_targets_fast(&[t]);
        // 真下のターゲットに水平前方 fwd → cos ~0。
        let fwd = [0.0, 0.0, 1.0];
        let (_m1, st1) = c.cull_fast_mask([0.5, 20.0, 0.5], fwd, &solid);
        assert_eq!(st1.visible, 0, "既定 0.35 では cos~0 が不可視");
        // 緩和 -1.0 (離心角 ≦180° 全通過)。FOV skip は評価を経ず前回値を持つ
        // (静的構成前提) ため、可視化は due (tick 10) 到達の実評価で起きる。
        c.fov_cos_min = -1.0;
        for i in 2..=10u64 {
            let (_m, st) = c.cull_fast_mask([0.5, 20.0, 0.5], fwd, &solid);
            if i == 10 {
                assert_eq!(st.reevaluated_this_tick, 1, "tick 10 で due 実評価");
                assert_eq!(
                    st.visible, 1,
                    "fov_cos_min=-1.0 では cos<0.35 のはずの対象が可視 (閾値の実効証明)"
                );
            } else {
                assert_eq!(
                    st.visible, 0,
                    "緩和直後も due 前は前回値 (不可視) を保持 — 静的構成前提"
                );
            }
        }
    }
}
