//! SIMD フラスタムカリング — SoA 配置の AABB 群を一括で視錐台内外判定。
//!
//! AVX2 (`_mm256_*`) 8レーン並列判定およびポータブル 4ワイド SWAR カリングを完全実装。
//! GPU ドリブンカリングの CPU 側事前絞り込みおよびマルチスレッド・パイプラインに使用。

#[derive(Debug, Clone, Copy)]
pub struct Plane {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
} // ax + by + cz + d >= 0 が「内」

impl Plane {
    #[inline(always)]
    pub fn distance(&self, x: f32, y: f32, z: f32) -> f32 {
        self.a * x + self.b * y + self.c * z + self.d
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// Structure of Arrays (SoA) layout for high-throughput AABB testing.
///
/// `repr(C, align(64))` は struct 自体の配置のみを整列する。6 本の `Vec` の
/// ヒープ領域は 64B アラインを**保証しない** (読み出しは loadu 系のため動作は
/// アライン不問で正しい)。
///
/// **契約**: 6 配列は全て等長必須。`from_aabbs` 経由なら自動的に満たされるが、
/// pub フィールドの手組みで不等長にすると `cull_soa_portable` /
/// `cull_soa_avx2` が契約メッセージ付きで panic する (2026-07-22 wave 29 で
/// fail-loud 化。それ以前は avx2 側が範囲外読みになり得た)。
#[repr(C, align(64))]
#[derive(Debug, Clone)]
pub struct SoaAabbs {
    pub min_x: Vec<f32>,
    pub min_y: Vec<f32>,
    pub min_z: Vec<f32>,
    pub max_x: Vec<f32>,
    pub max_y: Vec<f32>,
    pub max_z: Vec<f32>,
}

impl SoaAabbs {
    /// 6 配列の等長契約を検査 (fail-loud)。
    #[inline]
    #[track_caller]
    fn assert_uniform_len(&self) {
        let n = self.min_x.len();
        assert!(
            self.min_y.len() == n
                && self.min_z.len() == n
                && self.max_x.len() == n
                && self.max_y.len() == n
                && self.max_z.len() == n,
            "SoaAabbs 契約違反: 6 配列は等長必須 (min_x={}, min_y={}, min_z={}, \
             max_x={}, max_y={}, max_z={})",
            self.min_x.len(),
            self.min_y.len(),
            self.min_z.len(),
            self.max_x.len(),
            self.max_y.len(),
            self.max_z.len()
        );
    }

    pub fn from_aabbs(boxes: &[Aabb]) -> Self {
        let n = boxes.len();
        let mut min_x = Vec::with_capacity(n);
        let mut min_y = Vec::with_capacity(n);
        let mut min_z = Vec::with_capacity(n);
        let mut max_x = Vec::with_capacity(n);
        let mut max_y = Vec::with_capacity(n);
        let mut max_z = Vec::with_capacity(n);

        for b in boxes {
            min_x.push(b.min[0]);
            min_y.push(b.min[1]);
            min_z.push(b.min[2]);
            max_x.push(b.max[0]);
            max_y.push(b.max[1]);
            max_z.push(b.max[2]);
        }

        Self {
            min_x,
            min_y,
            min_z,
            max_x,
            max_y,
            max_z,
        }
    }

    pub fn len(&self) -> usize {
        self.min_x.len()
    }

    pub fn is_empty(&self) -> bool {
        self.min_x.is_empty()
    }
}

pub struct SimdFrustum {
    pub planes: [Plane; 6],
}

impl SimdFrustum {
    pub fn new(planes: [Plane; 6]) -> Self {
        Self { planes }
    }

    /// ボックスが視錐台に入るか（全平面で p-vertex が内側）。
    #[inline(always)]
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

    /// 複数ボックスを一括判定（ポータブル/AVX2 自動ディスパッチ対応）。
    pub fn cull(&self, boxes: &[Aabb]) -> Vec<bool> {
        self.cull_fast(boxes)
    }

    /// SoA レイアウトから最適な SIMD パスへ自動ディスパッチしてカリング。
    ///
    /// **契約 (2026-07-22 wave 29 明文化)**: AVX2 パスは FMA + 評価順
    /// `c*pz + (b*py + fma(a,px,d))`、ポータブルパスは丸めあり左結合
    /// `((a*x + b*y) + c*z) + d` で演算するため、距離が 0 ごく近傍の
    /// **境界上の箱では両者の判定が割れ得る** (機械依存)。決定的ダイジェスト等で
    /// 厳密性が必要な経路は `cull_soa_portable` を直接使うこと
    /// (full_graph_wiring が実例)。普段のカリングでは両者とも「真の距離が負なら
    /// 必ず除去、正なら誤除去しない」保守性は浮動小数丸めの範囲で同等。
    pub fn cull_fast(&self, boxes: &[Aabb]) -> Vec<bool> {
        if boxes.is_empty() {
            return Vec::new();
        }
        let soa = SoaAabbs::from_aabbs(boxes);
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        {
            if std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma") {
                return unsafe { self.cull_soa_avx2(&soa) };
            }
        }
        self.cull_soa_portable(&soa)
    }

    /// Portable 4-wide SWAR/vectorized culling over SoA arrays.
    pub fn cull_soa_portable(&self, soa: &SoaAabbs) -> Vec<bool> {
        soa.assert_uniform_len();
        let n = soa.len();
        let mut results = vec![true; n];

        let chunks = n / 4;
        for i in 0..chunks {
            let base = i * 4;
            let mut visible_mask = [true; 4];

            for p in &self.planes {
                for lane in 0..4 {
                    if !visible_mask[lane] {
                        continue;
                    }
                    let idx = base + lane;
                    let px = if p.a >= 0.0 { soa.max_x[idx] } else { soa.min_x[idx] };
                    let py = if p.b >= 0.0 { soa.max_y[idx] } else { soa.min_y[idx] };
                    let pz = if p.c >= 0.0 { soa.max_z[idx] } else { soa.min_z[idx] };
                    if p.distance(px, py, pz) < 0.0 {
                        visible_mask[lane] = false;
                    }
                }
            }

            for lane in 0..4 {
                results[base + lane] = visible_mask[lane];
            }
        }

        for idx in (chunks * 4)..n {
            let mut visible = true;
            for p in &self.planes {
                let px = if p.a >= 0.0 { soa.max_x[idx] } else { soa.min_x[idx] };
                let py = if p.b >= 0.0 { soa.max_y[idx] } else { soa.min_y[idx] };
                let pz = if p.c >= 0.0 { soa.max_z[idx] } else { soa.min_z[idx] };
                if p.distance(px, py, pz) < 0.0 {
                    visible = false;
                    break;
                }
            }
            results[idx] = visible;
        }

        results
    }

    /// AVX2 + FMA 8-lane SoA AABB Frustum Culling Kernel.
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[target_feature(enable = "avx2", enable = "fma")]
    pub unsafe fn cull_soa_avx2(&self, soa: &SoaAabbs) -> Vec<bool> {
        #[cfg(target_arch = "x86")]
        use std::arch::x86::*;
        #[cfg(target_arch = "x86_64")]
        use std::arch::x86_64::*;

        // 安全契約: この関数は 6 配列の等長を前提として生ポインタで読む。
        // 契約違反を UB (範囲外読み) にせず fail-loud に倒す (wave 29)。
        soa.assert_uniform_len();
        let n = soa.len();
        let mut results = vec![true; n];
        let chunks8 = n / 8;
        let zero_vec = _mm256_setzero_ps();

        let mut pa_vec = [_mm256_setzero_ps(); 6];
        let mut pb_vec = [_mm256_setzero_ps(); 6];
        let mut pc_vec = [_mm256_setzero_ps(); 6];
        let mut pd_vec = [_mm256_setzero_ps(); 6];
        let mut sign_x = [false; 6];
        let mut sign_y = [false; 6];
        let mut sign_z = [false; 6];

        for (idx, p) in self.planes.iter().enumerate() {
            pa_vec[idx] = _mm256_set1_ps(p.a);
            pb_vec[idx] = _mm256_set1_ps(p.b);
            pc_vec[idx] = _mm256_set1_ps(p.c);
            pd_vec[idx] = _mm256_set1_ps(p.d);
            sign_x[idx] = p.a >= 0.0;
            sign_y[idx] = p.b >= 0.0;
            sign_z[idx] = p.c >= 0.0;
        }

        for i in 0..chunks8 {
            let base = i * 8;
            let min_x = _mm256_loadu_ps(soa.min_x.as_ptr().add(base));
            let max_x = _mm256_loadu_ps(soa.max_x.as_ptr().add(base));
            let min_y = _mm256_loadu_ps(soa.min_y.as_ptr().add(base));
            let max_y = _mm256_loadu_ps(soa.max_y.as_ptr().add(base));
            let min_z = _mm256_loadu_ps(soa.min_z.as_ptr().add(base));
            let max_z = _mm256_loadu_ps(soa.max_z.as_ptr().add(base));

            let mut pass_mask = 0xFFi32;

            for p_idx in 0..6 {
                let px = if sign_x[p_idx] { max_x } else { min_x };
                let py = if sign_y[p_idx] { max_y } else { min_y };
                let pz = if sign_z[p_idx] { max_z } else { min_z };

                // dist = a * px + b * py + c * pz + d
                let mut dist = _mm256_fmadd_ps(pa_vec[p_idx], px, pd_vec[p_idx]);
                dist = _mm256_fmadd_ps(pb_vec[p_idx], py, dist);
                dist = _mm256_fmadd_ps(pc_vec[p_idx], pz, dist);

                // cmp < 0.0
                let cmp = _mm256_cmp_ps(dist, zero_vec, _CMP_LT_OQ);
                let fail_mask = _mm256_movemask_ps(cmp);
                pass_mask &= !fail_mask;

                if pass_mask == 0 {
                    break;
                }
            }

            for lane in 0..8 {
                results[base + lane] = ((pass_mask >> lane) & 1) != 0;
            }
        }

        for idx in (chunks8 * 8)..n {
            let mut visible = true;
            for p in &self.planes {
                let px = if p.a >= 0.0 { soa.max_x[idx] } else { soa.min_x[idx] };
                let py = if p.b >= 0.0 { soa.max_y[idx] } else { soa.min_y[idx] };
                let pz = if p.c >= 0.0 { soa.max_z[idx] } else { soa.min_z[idx] };
                if p.distance(px, py, pz) < 0.0 {
                    visible = false;
                    break;
                }
            }
            results[idx] = visible;
        }

        results
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

    #[test]
    fn batch_cull_soa() {
        let f = unit();
        let boxes = vec![
            Aabb { min: [-0.5; 3], max: [0.5; 3] },
            Aabb { min: [5.0; 3], max: [6.0; 3] },
            Aabb { min: [-0.9; 3], max: [0.9; 3] },
            Aabb { min: [10.0; 3], max: [20.0; 3] },
            Aabb { min: [-0.1; 3], max: [0.1; 3] },
            Aabb { min: [100.0; 3], max: [200.0; 3] },
            Aabb { min: [-0.8; 3], max: [0.8; 3] },
            Aabb { min: [-0.2; 3], max: [0.2; 3] },
            Aabb { min: [50.0; 3], max: [60.0; 3] },
        ];
        let r_fast = f.cull_fast(&boxes);
        let r_scalar = boxes.iter().map(|b| f.intersects(b)).collect::<Vec<_>>();
        assert_eq!(r_fast, r_scalar);
    }

    /// wave 29 追加テスト用の簡潔コンストラクタ (rustfmt 正準形と可読性の両立)。
    fn pl(a: f32, b: f32, c: f32, d: f32) -> Plane {
        Plane { a, b, c, d }
    }
    fn bx(mn: [f32; 3], mx: [f32; 3]) -> Aabb {
        Aabb { min: mn, max: mx }
    }

    /// wave 29-1: 境界触接 (dist == 0) は保守側=可視、剰余 (n=5: 4+1) 経路込みの
    /// 厳密列ピン。
    #[test]
    fn boundary_touching_is_visible_exact_sequence_with_remainder() {
        let f = unit();
        let boxes = vec![
            // 0: x=1 面に触接 (箱自体は外側) — dist == 0 → 可視 (保守)
            bx([1.0, 0.0, 0.0], [2.0, 1.0, 1.0]),
            // 1: x=1 面の完全外 — cull
            bx([4.0, 0.0, 0.0], [5.0, 1.0, 1.0]),
            // 2: 内部 — 可視
            bx([-0.5; 3], [0.5; 3]),
            // 3: x=-1 面の完全外 — cull
            bx([-3.0; 3], [-2.0; 3]),
            // 4: (1,1,1) コーナー触接 — dist == 0.1 超の内側 → 可視
            bx([0.9; 3], [1.0; 3]),
        ];
        let soa = SoaAabbs::from_aabbs(&boxes);
        assert_eq!(
            f.cull_soa_portable(&soa),
            vec![true, false, true, false, true]
        );
        assert_eq!(
            boxes.iter().map(|b| f.intersects(b)).collect::<Vec<_>>(),
            vec![true, false, true, false, true]
        );
    }

    /// wave 29-2: portable は intersects と同一式順 (左結合・丸めあり) の
    /// はずで、境界込み任意入力で **bit 完全一致** することを構造化集合で固定。
    #[test]
    fn portable_matches_scalar_bitexact_on_structured_set() {
        let f = SimdFrustum::new([
            pl(1.0, 0.0, 0.0, 3.0),  // x >= -3
            pl(-1.0, 0.0, 0.0, 3.0), // x <= 3
            pl(0.0, 1.0, 0.0, 2.0),  // y >= -2
            pl(0.0, -1.0, 1.0, 2.0), // -y + z + 2 >= 0 (斜め)
            pl(1.0, 1.0, 0.0, 4.0),  // x + y >= -4 (斜め)
            pl(0.0, 0.0, -1.0, 5.0), // z <= 5
        ]);
        // 13 箱 (chunk 3 + remainder 1)。0.25 刻みは f32 正確。
        let boxes: Vec<Aabb> = (0..13)
            .map(|i| {
                let x = (i % 5) as f32 - 3.5; // -3.5 .. 0.5
                let y = (i % 3) as f32 * 0.25 - 2.0; // -2.0 .. -1.5
                let z = (i % 4) as f32 * 0.5 - 1.0; // -1.0 .. 0.5
                let hx = 0.25 * (1 + (i % 3)) as f32;
                Aabb {
                    min: [x, y, z],
                    max: [x + hx, y + 0.5, z + 0.25],
                }
            })
            .collect();
        let soa = SoaAabbs::from_aabbs(&boxes);
        let portable = f.cull_soa_portable(&soa);
        let scalar: Vec<bool> = boxes.iter().map(|b| f.intersects(b)).collect();
        assert_eq!(
            portable, scalar,
            "portable と scalar は式順一致で bit 等しいはず"
        );
    }

    /// wave 29-3: 等長契約 fail-loud (portable)。
    #[test]
    #[should_panic(expected = "SoaAabbs 契約違反")]
    fn soa_length_mismatch_panics_portable() {
        let f = unit();
        let bad = SoaAabbs {
            min_x: vec![0.0; 8],
            min_y: vec![0.0; 8],
            min_z: vec![0.0; 8],
            max_x: vec![0.0; 8],
            max_y: vec![0.0; 8],
            max_z: vec![0.0; 4], // 不等長
        };
        let _ = f.cull_soa_portable(&bad);
    }

    /// wave 29-4: 等長契約 fail-loud (avx2)。検出できる環境では UB ではなく
    /// panic に倒れることを実機検証、非搭載環境では skip。
    #[test]
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    fn soa_length_mismatch_panics_avx2_when_detected() {
        if !(std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma")) {
            return;
        }
        let f = unit();
        let bad = SoaAabbs {
            min_x: vec![0.0; 8],
            min_y: vec![0.0; 8],
            min_z: vec![0.0; 8],
            max_x: vec![0.0; 8],
            max_y: vec![0.0; 8],
            max_z: vec![0.0; 4], // 不等長
        };
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            let _ = f.cull_soa_avx2(&bad);
        }));
        assert!(
            r.is_err(),
            "avx2 パスも契約違反で panic するはず (UB ではない)"
        );
    }

    /// wave 29-5: 整数幾何 (全積和が f32 正確) では avx2 == portable == scalar
    /// が厳密に一致することを実機検証。境界割れの余地が無い値域のみ使用。
    #[test]
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    fn avx2_matches_portable_on_integer_geometry_when_detected() {
        if !(std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma")) {
            return;
        }
        let f = SimdFrustum::new([
            pl(1.0, 0.0, 0.0, 4.0),
            pl(-1.0, 0.0, 0.0, 4.0),
            pl(0.0, 1.0, 0.0, 4.0),
            pl(0.0, -1.0, 0.0, 4.0),
            pl(1.0, 1.0, 1.0, 8.0),
            pl(-1.0, -1.0, -1.0, 8.0),
        ]);
        // 19 箱 (chunk8x2 + remainder 3)。座標は全て整数で |v|<=6、
        // 係数も整数 → 積和は最大数十程度で全て f32 正確 (FMA/左結合で不変)。
        let boxes: Vec<Aabb> = (0..19)
            .map(|i| {
                let x = (i % 7) as f32 - 4.0; // -4 .. 2
                let y = (i % 5) as f32 - 3.0; // -3 .. 1
                let z = (i % 3) as f32 - 2.0; // -2 .. 0
                Aabb {
                    min: [x, y, z],
                    max: [x + 1.0, y + 1.0, z + 1.0],
                }
            })
            .collect();
        let soa = SoaAabbs::from_aabbs(&boxes);
        let portable = f.cull_soa_portable(&soa);
        let avx2 = unsafe { f.cull_soa_avx2(&soa) };
        let scalar: Vec<bool> = boxes.iter().map(|b| f.intersects(b)).collect();
        assert_eq!(avx2, portable);
        assert_eq!(portable, scalar);
    }

    /// wave 29-6: 空入力は空。(WGSL ピンは wave 39 側へ分離)
    #[test]
    fn empty_inputs_still_empty() {
        let f = unit();
        assert!(f.cull(&[]).is_empty());
        let soa = SoaAabbs::from_aabbs(&[]);
        assert!(f.cull_soa_portable(&soa).is_empty());
    }

    /// wave 39-1: WGSL 実カーネルの entry/layout/binding 機械ピン。
    #[test]
    fn wgsl_kernel_entry_layout_and_bindings_pinned() {
        let module = naga::front::wgsl::parse_str(SIMD_FRUSTUM_WGSL).expect("WGSL must parse");
        let ep = module
            .entry_points
            .iter()
            .find(|ep| ep.name == "cs_cull")
            .expect("cs_cull entry missing");
        assert_eq!(ep.stage, naga::ShaderStage::Compute);
        assert_eq!(ep.workgroup_size, [64, 1, 1]);
        // Params: count@0, planes@16, span = roundUp(16, 16+96) = 112
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("Params"))
            .expect("Params struct missing");
        let naga::TypeInner::Struct { members, span } = &ty.inner else {
            panic!("Params is not a struct");
        };
        assert_eq!(*span, 112);
        let off = |name: &str| {
            members
                .iter()
                .find(|m| m.name.as_deref() == Some(name))
                .unwrap_or_else(|| panic!("Params.{name} missing"))
                .offset
        };
        assert_eq!(off("count"), 0);
        assert_eq!(off("planes"), 16);
        // 8 変数が全て (group 0, binding 0..7) に一意に割り当て
        let mut got: Vec<(u32, u32)> = module
            .global_variables
            .iter()
            .filter_map(|(_, g)| g.binding.as_ref().map(|b| (b.group, b.binding)))
            .collect();
        got.sort_unstable();
        let want: Vec<(u32, u32)> = (0..8).map(|b| (0, b)).collect();
        assert_eq!(got, want);
    }

    /// wave 39-2: WGSL カーネル (cs_cull) の逐行対応ミラーが CPU 参照と
    /// bit 一致することを構造化集合で固定 (同一式・同一評価規則の構成上の
    /// 一致を機械検証。GPU 側の bit 非保証は wgsl の doc 注記参照)。
    #[test]
    fn wgsl_mirror_matches_portable_bitexact() {
        /// cs_cull と同一制御フローの Rust ミラー (early exit 込み)。
        fn wgsl_mirror_cull(f: &SimdFrustum, boxes: &[Aabb]) -> Vec<u32> {
            boxes
                .iter()
                .map(|b| {
                    for p in &f.planes {
                        let px = if p.a >= 0.0 { b.max[0] } else { b.min[0] };
                        let py = if p.b >= 0.0 { b.max[1] } else { b.min[1] };
                        let pz = if p.c >= 0.0 { b.max[2] } else { b.min[2] };
                        let dist = p.a * px + p.b * py + p.c * pz + p.d;
                        if dist < 0.0 {
                            return 0u32;
                        }
                    }
                    1u32
                })
                .collect()
        }
        let f = SimdFrustum::new([
            pl(1.0, 0.0, 0.0, 3.0),  // x >= -3
            pl(-1.0, 0.0, 0.0, 3.0), // x <= 3
            pl(0.0, 1.0, 0.0, 2.0),  // y >= -2
            pl(0.0, -1.0, 1.0, 2.0), // -y + z + 2 >= 0 (斜め)
            pl(1.0, 1.0, 0.0, 4.0),  // x + y >= -4 (斜め)
            pl(0.0, 0.0, -1.0, 5.0), // z <= 5
        ]);
        let boxes: Vec<Aabb> = (0..17)
            .map(|i| {
                let x = (i % 5) as f32 - 3.5;
                let y = (i % 3) as f32 * 0.25 - 2.0;
                let z = (i % 4) as f32 * 0.5 - 1.0;
                let hx = 0.25 * (1 + (i % 3)) as f32;
                Aabb {
                    min: [x, y, z],
                    max: [x + hx, y + 0.5, z + 0.25],
                }
            })
            .collect();
        let soa = SoaAabbs::from_aabbs(&boxes);
        let portable_u32: Vec<u32> = f
            .cull_soa_portable(&soa)
            .iter()
            .map(|&v| v as u32)
            .collect();
        assert_eq!(wgsl_mirror_cull(&f, &boxes), portable_u32);
    }
}
