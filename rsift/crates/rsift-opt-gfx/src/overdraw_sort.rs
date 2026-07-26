//! Overdraw reduction — front-to-back sorting + early-Z simulation.
//!
//! On bandwidth-limited integrated GPUs, the cheapest fragment is the one never
//! shaded. Drawing opaque geometry nearest-first populates the depth buffer early, so
//! farther fragments fail the early-Z test and skip the pixel shader entirely. This
//! module sorts draws front-to-back and simulates the resulting early-Z pixel savings.

#[derive(Clone, Copy, Debug)]
pub struct DrawItem {
    pub center: [f32; 3],
    pub radius: f32,
    pub id: u32,
}

#[derive(Clone, Debug)]
pub struct OverdrawSorter;

impl OverdrawSorter {
    /// Sort draw indices nearest-first relative to `camera`.
    /// 【wave 130 ED-1】比較は `total_cmp` (全順序)。旧実装は
    /// `partial_cmp(...).unwrap()` で、NaN 中心・NaN カメラが 1 つでも
    /// 混入すると**パニック**だった (partial_cmp が None)。total_cmp は
    /// 有限値で partial_cmp と結果完全一致 (IEEE-754 bit 全順序、
    /// -0.0 < +0.0 も決定的)、NaN dist2 は常に最奥へ並ぶため入力破損時も
    /// 決定的で panic しない (M-4 系の堅牢化と同型)。テストで NaN 回帰を
    // pin。
    pub fn sort_front_to_back(items: &[DrawItem], camera: [f32; 3]) -> Vec<usize> {
        let mut order: Vec<usize> = (0..items.len()).collect();
        order.sort_by(|&a, &b| {
            let da = dist2(items[a].center, camera);
            let db = dist2(items[b].center, camera);
            da.total_cmp(&db)
        });
        order
    }

    /// 1-D early-Z simulation over `width` columns. Each draw is a screen span
    /// `[x0,x1)` at depth `d` (smaller = nearer). Returns how many pixels get shaded
    /// for the given draw `order` (each pixel is shaded whenever a covering span
    /// passes the depth test, i.e. is the current nearest).
    /// 【誠実注記 wave 130 ED-2】本シミュレータと `overdraw_saved` の実消費者は
    /// テスト/計測のみ (census grep: 本番呼出なし。実 draw への順序供給は
    /// `sort_front_to_back` — full_graph_wiring:704 の report.overdraw_order
    /// が担当)。計算量 O(width × spans)。保持: directive⑦ の計測器として
    /// (WGSL は gpu_runtime 実コンパイル検証資産)。
    pub fn early_z_shaded(width: usize, spans: &[(i32, i32, f32)], order: &[usize]) -> u32 {
        let mut depth: Vec<f32> = vec![f32::INFINITY; width];
        let mut shaded = 0u32;
        for &si in order {
            let (x0, x1, d) = spans[si];
            for x in x0.max(0)..x1.min(width as i32) {
                let x = x as usize;
                if d < depth[x] {
                    depth[x] = d;
                    shaded += 1;
                }
            }
        }
        shaded
    }

    /// Estimate overdraw reduction (shaded pixels saved) of front-to-back vs
    /// back-to-front ordering for the same spans.
    pub fn overdraw_saved(width: usize, spans: &[(i32, i32, f32)]) -> i32 {
        let n = spans.len();
        // 深度の浅い順 = front-to-back。その逆が back-to-front。
        // (呼び出し側の並びに依存しないよう、ここで必ず深度ソートする)
        let mut f2b: Vec<usize> = (0..n).collect();
        // ED-1: 上と同根の total_cmp 化 (NaN 深度でも決定的・panic なし)。
        f2b.sort_by(|&a, &b| spans[a].2.total_cmp(&spans[b].2));
        let b2f: Vec<usize> = f2b.iter().rev().cloned().collect();
        let s_f2b = Self::early_z_shaded(width, spans, &f2b);
        let s_b2f = Self::early_z_shaded(width, spans, &b2f);
        s_b2f as i32 - s_f2b as i32
    }

    pub fn wgsl_source(&self) -> &'static str {
        OVERDRAW_SORT_WGSL
    }
}

#[inline]
fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

pub const OVERDRAW_SORT_WGSL: &str = include_str!("../shaders/overdraw_sort.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_to_back_is_nearest_first() {
        let items = vec![
            DrawItem {
                center: [0.0, 0.0, -50.0],
                radius: 1.0,
                id: 0,
            },
            DrawItem {
                center: [0.0, 0.0, -5.0],
                radius: 1.0,
                id: 1,
            },
            DrawItem {
                center: [0.0, 0.0, -20.0],
                radius: 1.0,
                id: 2,
            },
        ];
        let order = OverdrawSorter::sort_front_to_back(&items, [0.0, 0.0, 0.0]);
        assert_eq!(order, vec![1, 2, 0]); // -5, -20, -50
    }

    #[test]
    fn front_to_back_reduces_shading() {
        // Two overlapping spans: near (d=1) and far (d=10), both cover columns 0..10.
        let spans = vec![(0, 10, 10.0), (0, 10, 1.0)];
        let saved = OverdrawSorter::overdraw_saved(10, &spans);
        assert!(
            saved > 0,
            "front-to-back should shade fewer pixels, saved={saved}"
        );
        // back-to-front shades 20 (both spans), front-to-back shades 10 (near only).
        let f2b = vec![1usize, 0];
        let b2f = vec![0usize, 1];
        assert_eq!(OverdrawSorter::early_z_shaded(10, &spans, &f2b), 10);
        assert_eq!(OverdrawSorter::early_z_shaded(10, &spans, &b2f), 20);
    }

    #[test]
    fn non_overlapping_spans_equal() {
        let spans = vec![(0, 5, 1.0), (5, 10, 1.0)];
        let s = OverdrawSorter::early_z_shaded(10, &spans, &[0, 1]);
        assert_eq!(s, 10);
    }

    #[test]
    fn sort_front_to_back_nan_inputs_stay_deterministic() {
        // ED-1: NaN 中心・NaN カメラを混ぜても panic せず決定的。
        // 旧実装 (partial_cmp().unwrap()) は本テストの全ケースで panic
        // (= 回帰検出)。
        let items = vec![
            DrawItem {
                center: [0.0, 0.0, f32::NAN],
                radius: 1.0,
                id: 0,
            },
            DrawItem {
                center: [0.0, 0.0, -5.0],
                radius: 1.0,
                id: 1,
            },
        ];
        let order = OverdrawSorter::sort_front_to_back(&items, [0.0, 0.0, 0.0]);
        assert_eq!(
            order,
            vec![1, 0],
            "有限 (dist2=25) が先、NaN dist2 は total_cmp で最奥"
        );
        // NaN カメラ: 全 dist2 が NaN → total_cmp で全て同順位扱い
        // (NaN == NaN は total_cmp で Equal) → 安定ソートで元順序保持。
        let order2 = OverdrawSorter::sort_front_to_back(&items, [f32::NAN; 3]);
        assert_eq!(order2, vec![0, 1], "全 NaN は安定ソートで元順序");
        // 同値の決定性: 2 回実行で同一結果 (テスト内部の同一性)。
        let a = OverdrawSorter::sort_front_to_back(&items, [0.0, 0.0, 0.0]);
        let b = OverdrawSorter::sort_front_to_back(&items, [0.0, 0.0, 0.0]);
        assert_eq!(a, b);
        // overdraw_saved 側も NaN 深度で panic しない (同根)。
        let saved = OverdrawSorter::overdraw_saved(4, &[(0, 4, f32::NAN), (0, 4, 1.0)]);
        let _ = saved; // panic しないこと自体が検証対象
    }
}
