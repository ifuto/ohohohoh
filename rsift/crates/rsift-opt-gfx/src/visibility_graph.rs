
//! Visibility Graph + Flood Fill - Sodium 0.5+発展
//! チャンク間可視をbitvec 64bitで表現、BFSで事前グラフ化

use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkNode { pub x: i32, pub z: i32 }

pub struct VisibilityGraph {
    adjacency: HashMap<ChunkNode, Vec<ChunkNode>>,
    visible_bits: HashMap<ChunkNode, u64>,
}

impl VisibilityGraph {
    pub fn new() -> Self { Self { adjacency: HashMap::new(), visible_bits: HashMap::new() } }

    pub fn add_edge(&mut self, a: ChunkNode, b: ChunkNode) {
        self.adjacency.entry(a).or_default().push(b);
        self.adjacency.entry(b).or_default().push(a);
    }

    /// カメラチャンクからBFSで可視を計算、距離と遮蔽を考慮
    pub fn flood_fill(&mut self, start: ChunkNode, max_dist: i32, is_opaque: impl Fn(ChunkNode) -> bool) -> Vec<ChunkNode> {
        let mut visited = HashMap::new();
        let mut queue = VecDeque::new();
        let mut result = Vec::new();
        queue.push_back((start, 0));
        visited.insert(start, true);
        while let Some((node, dist)) = queue.pop_front() {
            if dist > max_dist { continue; }
            if is_opaque(node) && dist > 0 { continue; }
            result.push(node);
            if let Some(neighbors) = self.adjacency.get(&node) {
                for &n in neighbors {
                    if !visited.contains_key(&n) {
                        visited.insert(n, true);
                        queue.push_back((n, dist+1));
                    }
                }
            }
        }
        result
    }

    pub fn is_visible(&self, from: ChunkNode, to: ChunkNode) -> bool {
        if let Some(bits) = self.visible_bits.get(&from) {
            // 簡易: x,z差をビット位置にマッピング
            let dx = (to.x - from.x + 32).clamp(0,63) as u64;
            let dz = (to.z - from.z + 32).clamp(0,63) as u64;
            let idx = (dx + dz*8) % 64;
            (bits >> idx) & 1 == 1
        } else { false }
    }
}
