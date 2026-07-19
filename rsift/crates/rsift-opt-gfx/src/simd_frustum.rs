//! SIMD 風フラスタムカリング — SoA 配置の AABB 群を一括で視錐台内外判定。
//!
//! 本番では `f32x4` で 4 ボックスを同時判定するが、ここではアルゴリズム本体
//! （p-vertex テスト）を実装する。GPU ドリブンカリングの CPU 側事前絞り込み等に使用。

#[derive(Debug, Clone, Copy)]
pub struct Plane {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
} // ax + by + cz + d >= 0 が「内」

impl Plane {
    pub fn distance(&self, x: f32, y: f32, z: f32) -> f32 {
        self.a * x + self.b * y + self.c * z + self.d
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

pub struct SimdFrustum {
    pub planes: [Plane; 6],
}

impl SimdFrustum {
    pub fn new(planes: [Plane; 6]) -> Self {
        Self { planes }
    }

    /// ボックスが視錐台に入るか（全平面で p-vertex が内側）。
    pub fn intersects(&self, b: &Aabb) -> bool {
        for p in &self.planes {
            let px = if p.a >= 0.0 { b.max[0] } else { b.min[0] };
            let py = if p.b >= 0.0 { b.max[1] } else { b.min[1] };
            let pz = if p.c >= 0.0 { b.max[2] } else { b.min[2] };
            if p.distance(px, py, pz) < 0.0 {
                return false;
            }
        }
        true
    }

    /// 複数ボックスを一括判定（本番は SIMD で 4 並列）。
    pub fn cull(&self, boxes: &[Aabb]) -> Vec<bool> {
        boxes.iter().map(|b| self.intersects(b)).collect()
    }
}

pub struct SimdFrustumCull;
impl SimdFrustumCull {
    pub fn wgsl_source(&self) -> &'static str {
        SIMD_FRUSTUM_WGSL
    }
}
pub const SIMD_FRUSTUM_WGSL: &str = include_str!("../shaders/simd_frustum.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    fn unit() -> SimdFrustum {
        // 原点中心の立方体視界（-1..1 各軸）を 6 平面で近似
        SimdFrustum::new([
            Plane { a: 1.0, b: 0.0, c: 0.0, d: 1.0 }, // x >= -1
            Plane { a: -1.0, b: 0.0, c: 0.0, d: 1.0 }, // x <= 1
            Plane { a: 0.0, b: 1.0, c: 0.0, d: 1.0 },
            Plane { a: 0.0, b: -1.0, c: 0.0, d: 1.0 },
            Plane { a: 0.0, b: 0.0, c: 1.0, d: 1.0 },
            Plane { a: 0.0, b: 0.0, c: -1.0, d: 1.0 },
        ])
    }

    #[test]
    fn inside_visible() {
        let f = unit();
        assert!(f.intersects(&Aabb {
            min: [-0.5; 3],
            max: [0.5; 3]
        }));
    }

    #[test]
    fn outside_culled() {
        let f = unit();
        assert!(!f.intersects(&Aabb {
            min: [2.0; 3],
            max: [3.0; 3]
        }));
    }

    #[test]
    fn batch_cull() {
        let f = unit();
        let r = f.cull(&[
            Aabb { min: [-0.5; 3], max: [0.5; 3] },
            Aabb { min: [5.0; 3], max: [6.0; 3] },
        ]);
        assert_eq!(r, vec![true, false]);
    }
}
