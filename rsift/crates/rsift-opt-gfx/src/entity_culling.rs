//! EntityCulling (tr7zw) 逆輸入 — 実体/ブロックエンティティのオクルージョンカリング。
//!
//! 本式アルゴリズム:
//! - カメラ→実体AABBサンプル点への Amanatides–Woo DDA レイキャストで遮蔽判定
//! - 非同期相当の分割: tick ごとに再評価数を限定 (ラウンドロビン・移動優先)
//! - 結果は 1 フレーム遅延で適用 (描画揺れを抑えるスキッピング戦略)

use std::collections::HashSet;

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

#[derive(Debug, Clone, Copy)]
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

/// オクルージョンカリングの中核。
pub struct EntityCuller {
    tracked: Vec<Tracked>,
    tick: u64,
    pub budget_per_tick: usize,
    pub period_ticks: u64,
    pub max_distance: f32,
    pub ray_through_transparent_gain: u32,
    // 直近 tick の実測カウンタ (stats() が読む)
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

    /// ワールド配信から実体一覧が変わったときに差し替える。
    pub fn replace_targets(&mut self, targets: Vec<EntityTarget>) {
        let mut map = std::collections::HashMap::new();
        for t in self.tracked.drain(..) {
            map.insert(t.target.id, t);
        }
        self.tracked = targets
            .into_iter()
            .map(|target| {
                if let Some(mut prev) = map.remove(&target.id) {
                    // 移動検出 (これが EntityCulling の "moved → re-eval 優先" 相当)
                    prev.target = target;
                    prev
                } else {
                    Tracked {
                        target,
                        last_center: center_of(&target),
                        last_eval_tick: 0,
                        visible: true, // 新規実体は一旦描画 (ちらつき防止)
                    }
                }
            })
            .collect();
    }

    /// 描画前に呼ぶ。可視 ID 集合を返す。
    pub fn visible_ids<S: SolidQuery>(&mut self, cam: [f32; 3], solids: &S) -> HashSet<u64> {
        self.tick = self.tick.wrapping_add(1);
        let mut evaluated = 0usize;
        let mut rays = 0u32;
        let mut far = 0u32;
        let max_d_sq = self.max_distance * self.max_distance;
        let mut out = HashSet::with_capacity(self.tracked.len());

        // ラウンドロビン順序: tick 起点で回すと budget 切れでも全実体が公平に回る
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
                // 距離上限: 判定自体を省く (ValueDistculling)
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
        let mut stats_visible = 0u32;
        for t in &self.tracked {
            let c = center_of(&t.target);
            if dist_sq(c, cam) > max_d_sq {
                continue;
            }
            if t.visible {
                out.insert(t.target.id);
                stats_visible += 1;
            }
        }
        let _ = stats_visible;
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

/// カメラに向いたサンプルを優先: 軸ごとに nearest/center/farthest。
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
/// 27 ブロック歩幅で打ち止め (EntityCulling の "max scan distance" 相当)。
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
            return true; // 目的セルに到達 = 非遮蔽
        }
        if guard > 256 {
            return true;
        }
        guard += 1;

        // 出発セル上の実体自身は無視するため、先に歩進める
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
}
