//! Visibility Graph + Flood Fill - Sodium 0.5+発展
//! チャンク間可視をBFSで事前グラフ化

use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkNode {
    pub x: i32,
    pub z: i32,
}

/// キャッシュする可視集合 (start チャンク単位) の上限。超過時は全破棄して再構築する
/// (長時間プレイでカメラが移動し続けてもメモリが単調増大しないようにする番兵)。
const VISIBLE_CACHE_CAP: usize = 1024;

pub struct VisibilityGraph {
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

    pub fn add_edge(&mut self, a: ChunkNode, b: ChunkNode) {
        self.adjacency.entry(a).or_default().push(b);
        self.adjacency.entry(b).or_default().push(a);
    }

    /// カメラチャンクからBFSで可視を計算、距離と遮蔽を考慮。
    /// `is_opaque(node)` が真のノードは結果に含めず、それ以降の伝播も遮断する
    /// (dist=0 の始点自身は opaque 判定から除外)。到達集合は is_visible 用にキャッシュする。
    pub fn flood_fill(
        &mut self,
        start: ChunkNode,
        max_dist: i32,
        is_opaque: impl Fn(ChunkNode) -> bool,
    ) -> Vec<ChunkNode> {
        let mut visited = HashMap::new();
        let mut queue = VecDeque::new();
        let mut result = Vec::new();
        queue.push_back((start, 0));
        visited.insert(start, true);
        while let Some((node, dist)) = queue.pop_front() {
            if dist > max_dist {
                continue;
            }
            if is_opaque(node) && dist > 0 {
                continue;
            }
            result.push(node);
            if let Some(neighbors) = self.adjacency.get(&node) {
                for &n in neighbors {
                    if !visited.contains_key(&n) {
                        visited.insert(n, true);
                        queue.push_back((n, dist + 1));
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
}
