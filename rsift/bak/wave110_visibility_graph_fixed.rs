//! Visibility Graph + Flood Fill - Sodium 0.5+発展
//! チャンク間可視をBFSで事前グラフ化。
//!
//! ## 意味論 (監査 2026-07-25 DJ-4 確定)
//! `flood_fill(start, d, is_opaque)` の返却集合は、opaque ノード (始点を除く)
//! を「終点にはなれるが通過点にはなれない」頂点として扱う **遮断測地球**
//! { v : δ(start, v) ≤ d } であり、これは G を頂点集合 (V \ Opaque) ∪ {start}
//! に制限した誘導部分グラフでの半径 d の BFS 球に厳密に一致する
//! (参照実装との差分ファズで実証済み)。opaque ノードは結果に含まれず、
//! その先への伝播も遮断される。

use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkNode {
    pub x: i32,
    pub z: i32,
}

/// キャッシュする可視集合 (start チャンク単位) の上限。超過時は全破棄して再構築する
/// (長時間プレイでカメラが移動し続けてもメモリが単調増大しないようにする番兵)。
///
/// 不変量: `visible_sets.len() <= VISIBLE_CACHE_CAP` が insert 後も常時成立する。
/// 全破棄は直近利用キャッシュも巻き込むため、cap を超える数の start を巡回する
/// ワークロードでは is_visible が周期ごとに一時 false に戻る (再 flood で回復。
/// 誤った true を返すことはない)。
const VISIBLE_CACHE_CAP: usize = 1024;

pub struct VisibilityGraph {
    /// 単純無向グラフとしての隣接リスト (多重辺は保持しない)。
    /// 【監査 2026-07-25 DJ-1】旧実装は `add_edge` 呼出毎に無条件 push する
    /// multigraph で、フレーム毎に同一辺を再登録する消費者
    /// (full_graph_wiring::tick_world は render_pipeline から毎フレーム呼出)
    /// 経由で隣接 Vec が単調増大し、flood_fill の走査幅だけが漸次増える
    /// 構造 (結果は visited 抑止により正しいままだがメモリと時間を浪費する)
    /// だったため、冪等化で根治した。
    adjacency: HashMap<ChunkNode, Vec<ChunkNode>>,
    /// flood_fill(start, ...) の到達集合を start チャンク単位でキャッシュ。
    /// 旧 `visible_bits: HashMap<ChunkNode, u64>` は書き込み経路が一切存在せず
    /// is_visible が恒に false を返すセマンティックスタブだった (監査 2026-07-21)。
    /// さらに旧ビット写像 `(dx + dz*8) % 64` は (dx=0,dz=1) と (dx=8,dz=0) が
    /// 衝突する破綻写像だったため、bitvec 表現は撤去し実集合キャッシュに置換した。
    visible_sets: HashMap<ChunkNode, HashSet<ChunkNode>>,
}

impl VisibilityGraph {
    pub fn new() -> Self {
        Self {
            adjacency: HashMap::new(),
            visible_sets: HashMap::new(),
        }
    }

    /// 無向辺 a-b を 1 本追加する。冪等: 同一辺の再登録・逆向き登録は無視
    /// され、自己ループは 1 件の自己参照として正規化される (多重辺は BFS
    /// 結果に一切寄与せずメモリと走査幅だけを浪費するため保持しない。DJ-1)。
    pub fn add_edge(&mut self, a: ChunkNode, b: ChunkNode) {
        let ab = self.adjacency.entry(a).or_default();
        if !ab.contains(&b) {
            ab.push(b);
        }
        let ba = self.adjacency.entry(b).or_default();
        if !ba.contains(&a) {
            ba.push(a);
        }
    }

    /// カメラチャンクからBFSで可視を計算、距離と遮蔽を考慮。
    /// `is_opaque(node)` が真のノードは結果に含めず、それ以降の伝播も遮断する
    /// (dist=0 の始点自身は opaque 判定から除外)。到達集合は is_visible 用に
    /// キャッシュする。
    ///
    /// ## 契約 (監査 2026-07-25 DJ-4)
    /// * 返却 Vec は BFS 訪問順 (A. 始点 → B. 近い層から、同層内は add_edge
    ///   の登録順) で決定的かつ重複なし。
    /// * `is_opaque` は各呼出につき各ノード高々 1 回だけ評価される。始点にも
    ///   評価されるが、その判定値は結果に用いられない (始点免除)。
    /// * `max_dist < 0` のとき結果は空 (キャッシュには start の空集合が残る)。
    /// * キャッシュが満杯 (`VISIBLE_CACHE_CAP`) のとき新規 start が来ると
    ///   キャッシュを全破棄し、その start のみを再登録する (is_visible 参照)。
    pub fn flood_fill(
        &mut self,
        start: ChunkNode,
        max_dist: i32,
        is_opaque: impl Fn(ChunkNode) -> bool,
    ) -> Vec<ChunkNode> {
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        let mut result = Vec::new();
        queue.push_back((start, 0));
        visited.insert(start);
        while let Some((node, dist)) = queue.pop_front() {
            if dist > max_dist {
                continue;
            }
            if is_opaque(node) && dist > 0 {
                continue;
            }
            result.push(node);
            // dist の子は dist+1: max_dist ちょうどのノードを展開しても子は
            // 全て次の pop で即棄却されるため、自明に到達不能な殻 (max_dist+1
            // 層) は積まない (DJ-3)。結果集合・is_opaque 呼出集合・キャッシュ
            // 内容は旧実装と bit 完全一致で、queue 交通量のみ低減する。
            if dist < max_dist {
                if let Some(neighbors) = self.adjacency.get(&node) {
                    for &n in neighbors {
                        if !visited.contains(&n) {
                            visited.insert(n);
                            queue.push_back((n, dist + 1));
                        }
                    }
                }
            }
        }
        if !self.visible_sets.contains_key(&start) && self.visible_sets.len() >= VISIBLE_CACHE_CAP {
            self.visible_sets.clear();
        }
        self.visible_sets
            .insert(start, result.iter().copied().collect());
        result
    }

    /// `from` を始点とする直近の flood_fill 到達集合に `to` が含まれるか。
    /// flood_fill 未実行の from については false (キャッシュ無し) を返す。
    /// `VISIBLE_CACHE_CAP` 超過時の全破棄 (eviction) により、直近で flood
    /// した from についても一時 false に戻りうる (再 flood で回復する)。
    pub fn is_visible(&self, from: ChunkNode, to: ChunkNode) -> bool {
        self.visible_sets
            .get(&from)
            .map(|set| set.contains(&to))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(x: i32, z: i32) -> ChunkNode {
        ChunkNode { x, z }
    }

    /// g×g の 4 近傍グリッドグラフを構築。
    fn grid(g: i32) -> VisibilityGraph {
        let mut vg = VisibilityGraph::new();
        for z in 0..g {
            for x in 0..g {
                if x + 1 < g {
                    vg.add_edge(n(x, z), n(x + 1, z));
                }
                if z + 1 < g {
                    vg.add_edge(n(x, z), n(x, z + 1));
                }
            }
        }
        vg
    }

    fn sorted(v: Vec<ChunkNode>) -> Vec<ChunkNode> {
        let mut v = v;
        v.sort_by_key(|c| (c.x, c.z));
        v
    }

    #[test]
    fn flood_fill_corner_max_dist_matches_manhattan_set() {
        let mut vg = grid(4);
        // (0,0) から max_dist=2、遮蔽なし → マンハッタン距離 <= 2 の 6 ノード。
        let got = vg.flood_fill(n(0, 0), 2, |_| false);
        let expect = vec![n(0, 0), n(1, 0), n(2, 0), n(0, 1), n(1, 1), n(0, 2)];
        assert_eq!(sorted(got), sorted(expect));
    }

    #[test]
    fn flood_fill_max_dist_zero_returns_start_only() {
        let mut vg = grid(3);
        let got = vg.flood_fill(n(1, 1), 0, |_| false);
        assert_eq!(got, vec![n(1, 1)]);
    }

    #[test]
    fn opaque_node_blocks_result_and_propagation() {
        let mut vg = grid(4);
        // (1,0) を壁にする: (0,0)→(1,0) 経路は遮断。(0,1) 側は dist<=2 で伝播。
        let opaque = |c: ChunkNode| c == n(1, 0);
        let got = vg.flood_fill(n(0, 0), 2, opaque);
        // (1,0) 自身は結果に含まれず、その背後 (2,0) にも dist 経由で届かない。
        let expect = vec![n(0, 0), n(0, 1), n(0, 2), n(1, 1)];
        assert_eq!(sorted(got), sorted(expect));
    }

    #[test]
    fn opaque_start_node_is_still_included() {
        let mut vg = grid(2);
        // dist=0 の始点は opaque 扱いしない設計。
        let got = vg.flood_fill(n(0, 0), 0, |_| true);
        assert_eq!(got, vec![n(0, 0)]);
    }

    #[test]
    fn is_visible_reflects_flood_cache() {
        let mut vg = grid(3);
        // flood 前: キャッシュ無し → 恒に false。
        assert!(!vg.is_visible(n(0, 0), n(0, 0)));
        let _ = vg.flood_fill(n(0, 0), 2, |_| false);
        assert!(vg.is_visible(n(0, 0), n(0, 0)));
        assert!(vg.is_visible(n(0, 0), n(1, 1)));
        assert!(!vg.is_visible(n(0, 0), n(2, 2))); // 距離超過
        assert!(!vg.is_visible(n(1, 1), n(0, 0))); // 別 start のキャッシュは無い
                                                   // 別 start で flood するとその start でも参照可能になる。
        let _ = vg.flood_fill(n(2, 2), 1, |_| false);
        assert!(vg.is_visible(n(2, 2), n(1, 2)));
        assert!(!vg.is_visible(n(2, 2), n(0, 0)));
    }

    #[test]
    fn flood_fill_on_disconnected_graph_yields_start_component_only() {
        let mut vg = VisibilityGraph::new();
        vg.add_edge(n(0, 0), n(1, 0));
        let got = vg.flood_fill(n(0, 0), 8, |_| false);
        assert_eq!(sorted(got), sorted(vec![n(0, 0), n(1, 0)]));
    }

    // ------------------------------------------------------------ 監査 2026-07-25 DJ追加分

    /// DJ-1: add_edge は冪等。重複登録・逆向き登録・自己ループは隣接構造を
    /// 変えず、flood 結果 (BFS 訪問順 Vec) も単一登録と完全一致する。
    #[test]
    fn add_edge_is_idempotent_and_self_loop_safe() {
        let mut single = VisibilityGraph::new();
        single.add_edge(n(0, 0), n(1, 0));
        let mut dup = VisibilityGraph::new();
        for _ in 0..3 {
            dup.add_edge(n(0, 0), n(1, 0));
            dup.add_edge(n(1, 0), n(0, 0));
        }
        dup.add_edge(n(0, 0), n(0, 0)); // 自己ループ → 1 件の自己参照に正規化
        assert_eq!(dup.adjacency[&n(0, 0)], vec![n(1, 0), n(0, 0)]);
        assert_eq!(dup.adjacency[&n(1, 0)], vec![n(0, 0)]);
        let a = single.flood_fill(n(0, 0), 4, |_| false);
        let b = dup.flood_fill(n(0, 0), 4, |_| false);
        assert_eq!(a, b, "重複登録は BFS 結果 (順序含む) に影響しない");
    }

    /// DJ-5: 遮蔽なし flood は閉形式の球と一致する。
    /// 角始点 (g > d でクリップされない): |{x+z<=d}| = (d+1)(d+2)/2。
    /// 中央始点: 独立二重ループのダイヤモンド数え上げ (境界クリップ込み)。
    #[test]
    fn flood_fill_matches_closed_form_balls() {
        let d_max = 6i32;
        for d in 0..=d_max {
            let mut corner = grid(9);
            let got = corner.flood_fill(n(0, 0), d, |_| false);
            assert_eq!(got.len(), ((d + 1) * (d + 2) / 2) as usize, "corner d");
            let mut center = grid(9);
            let c = 4i32;
            let expect = (0..9)
                .flat_map(|x| (0..9).map(move |z| (x, z)))
                .filter(|&(x, z)| (x - c).abs() + (z - c).abs() <= d)
                .count();
            let got2 = center.flood_fill(n(c, c), d, |_| false);
            assert_eq!(got2.len(), expect, "center d");
        }
    }

    /// DJ-5: bench (wide_static_bench) の opaque=0 行の閉形式値をピン。
    /// 角 d=6 → 28, 中央 g=8 → 59 (角クリップ), 中央 g=16 → 85 (=1+2d(d+1))。
    #[test]
    fn bench_shape_closed_forms_are_pinned() {
        let mut g8c = grid(8);
        assert_eq!(g8c.flood_fill(n(0, 0), 6, |_| false).len(), 28);
        let mut g8m = grid(8);
        assert_eq!(g8m.flood_fill(n(4, 4), 6, |_| false).len(), 59);
        let mut g16m = grid(16);
        assert_eq!(g16m.flood_fill(n(8, 8), 6, |_| false).len(), 85);
    }

    /// DJ-4 定理 pin: flood_fill の結果集合は、opaque ノード (始点を除く) を
    /// 取り除いた誘導部分グラフでの半径 max_dist の層別 BFS 球に等しい。
    /// 参照実装 (pruned 隣接の再構成 + 層ループ) は独立に記述した第二実装。
    #[test]
    fn flood_fill_equals_vertex_pruned_geodesic_ball() {
        let mut s = 0x1234_5678_9abc_def0u64;
        let mut rnd = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        let nodes: Vec<ChunkNode> = (0..24).map(|i| n(i % 6, i / 6)).collect();
        for _trial in 0..20 {
            let mut vg = VisibilityGraph::new();
            let mut edges: Vec<(usize, usize)> = Vec::new();
            for i in 0..24usize {
                for j in (i + 1)..24usize {
                    if rnd() % 100 < 18 {
                        vg.add_edge(nodes[i], nodes[j]);
                        edges.push((i, j));
                    }
                }
            }
            let start = nodes[rnd() as usize % 24];
            // 始点免除でなく通常の遮断球を比較するため、始点は opaque にしない。
            let opaque: HashSet<ChunkNode> = (0..3)
                .map(|_| nodes[rnd() as usize % 24])
                .filter(|&c| c != start)
                .collect();
            let max_dist = (rnd() % 5) as i32;
            let got = vg.flood_fill(start, max_dist, |c| opaque.contains(&c));
            // 参照: 残留頂点のみの隣接を再構成し、始点から層別に膨張。
            let pruned = |c: ChunkNode| c == start || !opaque.contains(&c);
            let mut padj: HashMap<ChunkNode, Vec<ChunkNode>> = HashMap::new();
            for &(i, j) in &edges {
                let (a, b) = (nodes[i], nodes[j]);
                if pruned(a) && pruned(b) {
                    padj.entry(a).or_default().push(b);
                    padj.entry(b).or_default().push(a);
                }
            }
            let mut reached: HashSet<ChunkNode> = HashSet::new();
            let mut frontier = vec![start];
            let mut dist = 0;
            while !frontier.is_empty() && dist <= max_dist {
                let mut next = Vec::new();
                for &c in &frontier {
                    if reached.insert(c) {
                        if let Some(ns) = padj.get(&c) {
                            next.extend_from_slice(ns);
                        }
                    }
                }
                frontier = next;
                dist += 1;
            }
            let got_set: HashSet<ChunkNode> = got.iter().copied().collect();
            assert_eq!(got_set, reached, "遮断測地球との不一致");
            assert_eq!(got_set.len(), got.len(), "結果 Vec に重複あり");
        }
    }

    /// DJ-4 契約 pin: is_opaque は各ノード高々 1 回だけ評価される
    /// (始点にも 1 回評価されるが判定値は結果に用いられない)。
    #[test]
    fn is_opaque_called_at_most_once_per_node_including_start() {
        let calls: std::cell::RefCell<HashMap<ChunkNode, u32>> = Default::default();
        let mut vg = grid(4);
        let opaque = |c: ChunkNode| {
            *calls.borrow_mut().entry(c).or_insert(0) += 1;
            c == n(3, 3)
        };
        let got = vg.flood_fill(n(0, 0), 2, opaque);
        let calls = calls.borrow();
        assert_eq!(
            calls.get(&n(0, 0)),
            Some(&1),
            "始点も 1 回評価される (判定は不使用)"
        );
        for k in calls.values() {
            assert!(*k <= 1, "同一ノードの複数回評価: {k} 回");
        }
        // (3,3) は opaque かつ dist=6>2: 評価自体が発生せず結果にも入らない。
        assert!(!got.contains(&n(3, 3)));
    }

    /// DJ-4 契約 pin: max_dist < 0 は空結果 + start の空集合キャッシュ。
    #[test]
    fn negative_max_dist_yields_empty_and_caches_empty() {
        let mut vg = grid(3);
        let got = vg.flood_fill(n(1, 1), -1, |_| false);
        assert!(got.is_empty(), "max_dist<0 は空結果");
        assert!(!vg.is_visible(n(1, 1), n(1, 1)), "空集合がキャッシュされる");
    }

    /// DJ-4 契約 pin: キャッシュ不変量 len<=CAP。CAP+1 個目の新規 start で
    /// 全破棄 → 当該 start のみ残存 (is_visible は一時 false、再 flood で回復)。
    #[test]
    fn visible_cache_cap_evicts_all_then_recovers_single() {
        let mut vg = grid(34); // 34*34 = 1156 >= CAP+1 distinct starts
        let mut starts = Vec::new();
        for i in 0..(VISIBLE_CACHE_CAP + 1) as i32 {
            let c = n(i % 34, i / 34);
            starts.push(c);
            let r = vg.flood_fill(c, 0, |_| false);
            assert_eq!(r, vec![c]);
        }
        assert!(
            !vg.is_visible(starts[0], starts[0]),
            "eviction で先頭 start は一時 false"
        );
        assert!(
            vg.is_visible(starts[VISIBLE_CACHE_CAP], starts[VISIBLE_CACHE_CAP]),
            "全破棄後は当該 start のみ残存"
        );
        let _ = vg.flood_fill(starts[0], 0, |_| false);
        assert!(vg.is_visible(starts[0], starts[0]), "再 flood で回復");
    }

    /// DJ-4 契約 pin: 同一登録順の 2 グラフは結果 Vec (BFS 訪問順) まで一致。
    #[test]
    fn flood_result_order_is_deterministic_given_registration_order() {
        let mut a = grid(5);
        let mut b = grid(5);
        let ra = a.flood_fill(n(2, 2), 2, |c| c == n(2, 3));
        let rb = b.flood_fill(n(2, 2), 2, |c| c == n(2, 3));
        assert_eq!(ra, rb);
    }
}
