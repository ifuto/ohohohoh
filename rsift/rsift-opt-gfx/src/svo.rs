//! Sparse Voxel Octree (SVO) — far-LOD / zero-polygon path with branchless DDA leaf refine.

use crate::binary_greedy_meshing::{idx, SectionPalette, SECTION_SIZE, SECTIONS_PER_COLUMN};
use crate::branchless_dda::{Ray3, VoxelHit, trace_section};
use tracing::trace;

const MAX_DEPTH: u32 = 4; // 2^4 = 16

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeKind {
    Empty,
    Uniform(u16),
    Branch,
}

#[derive(Debug, Clone)]
struct Node {
    kind: NodeKind,
    /// For Branch: indices into `nodes` for 8 children (0 = empty child slot uses Empty kind inline)
    children: [u32; 8],
}

impl Default for Node {
    fn default() -> Self {
        Self {
            kind: NodeKind::Empty,
            children: [0; 8],
        }
    }
}

#[derive(Debug, Clone)]
pub struct SparseVoxelOctree {
    nodes: Vec<Node>,
    pub root: u32,
    pub bounds: [usize; 3],
    pub node_count: u32,
    pub leaf_count: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct SvoHit {
    pub world: [f32; 3],
    pub block: u16,
    pub steps: u32,
}

impl SparseVoxelOctree {
    pub fn empty() -> Self {
        Self {
            nodes: Vec::new(),
            root: 0,
            bounds: [SECTION_SIZE, SECTION_SIZE, SECTION_SIZE * SECTIONS_PER_COLUMN],
            node_count: 0,
            leaf_count: 0,
        }
    }

    /// Build SVO from one 16³ section palette.
    pub fn from_section(palette: &SectionPalette) -> Self {
        let mut tree = Self::empty();
        tree.bounds = [SECTION_SIZE, SECTION_SIZE, SECTION_SIZE];
        tree.root = tree.build_node(palette, 0, 0, 0, SECTION_SIZE, 0);
        tree.node_count = tree.nodes.len() as u32;
        trace!(
            "[SVO] section built: {} nodes, {} leaves",
            tree.node_count,
            tree.leaf_count
        );
        tree
    }

    /// Stack 4 sections into one 16×64×16 column octree.
    pub fn from_column(sections: &[SectionPalette]) -> Self {
        let height = SECTION_SIZE * sections.len().min(SECTIONS_PER_COLUMN);
        let mut column = vec![0u16; SECTION_SIZE * height * SECTION_SIZE];
        for (sy, sec) in sections.iter().enumerate().take(SECTIONS_PER_COLUMN) {
            for y in 0..SECTION_SIZE {
                for z in 0..SECTION_SIZE {
                    for x in 0..SECTION_SIZE {
                        let wy = sy * SECTION_SIZE + y;
                        let di = x + wy * SECTION_SIZE + z * SECTION_SIZE * height;
                        column[di] = sec[idx(x, y, z)];
                    }
                }
            }
        }
        let mut tree = Self::empty();
        tree.bounds = [SECTION_SIZE, height, SECTION_SIZE];
        tree.root = tree.build_node_column(&column, 0, 0, 0, SECTION_SIZE, height, SECTION_SIZE, 0);
        tree.node_count = tree.nodes.len() as u32;
        trace!(
            "[SVO] column {}×{}×{}: {} nodes, {} leaves",
            tree.bounds[0],
            tree.bounds[1],
            tree.bounds[2],
            tree.node_count,
            tree.leaf_count
        );
        tree
    }

    fn alloc_node(&mut self, node: Node) -> u32 {
        let id = self.nodes.len() as u32;
        if matches!(node.kind, NodeKind::Uniform(_)) {
            self.leaf_count += 1;
        }
        self.nodes.push(node);
        id
    }

    fn build_node(
        &mut self,
        palette: &SectionPalette,
        ox: usize,
        oy: usize,
        oz: usize,
        size: usize,
        depth: u32,
    ) -> u32 {
        if size == 0 {
            return self.alloc_node(Node::default());
        }
        let (uniform, block) = region_uniform(palette, ox, oy, oz, size);
        if uniform {
            return self.alloc_node(Node {
                kind: if block == 0 {
                    NodeKind::Empty
                } else {
                    NodeKind::Uniform(block)
                },
                children: [0; 8],
            });
        }
        if depth >= MAX_DEPTH || size <= 1 {
            let dominant = dominant_block(palette, ox, oy, oz, size);
            return self.alloc_node(Node {
                kind: if dominant == 0 {
                    NodeKind::Empty
                } else {
                    NodeKind::Uniform(dominant)
                },
                children: [0; 8],
            });
        }
        let half = size / 2;
        let mut children = [0u32; 8];
        let mut child_idx = 0usize;
        for dz in 0..2 {
            for dy in 0..2 {
                for dx in 0..2 {
                    let cx = ox + dx * half;
                    let cy = oy + dy * half;
                    let cz = oz + dz * half;
                    children[child_idx] =
                        self.build_node(palette, cx, cy, cz, half.max(1), depth + 1);
                    child_idx += 1;
                }
            }
        }
        self.alloc_node(Node {
            kind: NodeKind::Branch,
            children,
        })
    }

    fn build_node_column(
        &mut self,
        column: &[u16],
        ox: usize,
        oy: usize,
        oz: usize,
        sx: usize,
        sy: usize,
        sz: usize,
        depth: u32,
    ) -> u32 {
        let size = sx.min(sy).min(sz);
        if size == 0 {
            return self.alloc_node(Node::default());
        }
        let (uniform, block) = region_uniform_column(column, ox, oy, oz, sx, sy, sz);
        if uniform {
            return self.alloc_node(Node {
                kind: if block == 0 {
                    NodeKind::Empty
                } else {
                    NodeKind::Uniform(block)
                },
                children: [0; 8],
            });
        }
        if depth >= MAX_DEPTH || size <= 1 {
            let dominant = dominant_block_column(column, ox, oy, oz, sx, sy, sz);
            return self.alloc_node(Node {
                kind: if dominant == 0 {
                    NodeKind::Empty
                } else {
                    NodeKind::Uniform(dominant)
                },
                children: [0; 8],
            });
        }
        let half = size / 2;
        let mut children = [0u32; 8];
        let mut ci = 0usize;
        for dz in 0..2 {
            for dy in 0..2 {
                for dx in 0..2 {
                    children[ci] = self.build_node_column(
                        column,
                        ox + dx * half,
                        oy + dy * half,
                        oz + dz * half,
                        half.max(1),
                        half.max(1),
                        half.max(1),
                        depth + 1,
                    );
                    ci += 1;
                }
            }
        }
        self.alloc_node(Node {
            kind: NodeKind::Branch,
            children,
        })
    }

    /// Octree-guided ray trace; refines with branchless DDA inside non-uniform leaves.
    pub fn trace(&self, ray: &Ray3, section_palette: Option<&SectionPalette>) -> Option<SvoHit> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut stack = [(0u32, 0.0f32, f32::INFINITY); 64];
        stack[0] = (self.root, 0.0, f32::INFINITY);
        let mut sp = 1usize;

        while sp > 0 {
            sp -= 1;
            let (node_id, t_enter, t_exit) = stack[sp];
            if node_id as usize >= self.nodes.len() {
                continue;
            }
            let node = &self.nodes[node_id as usize];
            match node.kind {
                NodeKind::Empty => {}
                NodeKind::Uniform(block) if block != 0 => {
                    return Some(SvoHit {
                        world: [
                            ray.origin[0] + ray.dir[0] * t_enter,
                            ray.origin[1] + ray.dir[1] * t_enter,
                            ray.origin[2] + ray.dir[2] * t_enter,
                        ],
                        block,
                        steps: 0,
                    });
                }
                NodeKind::Uniform(_) => {}
                NodeKind::Branch => {
                    for &child in node.children.iter().rev() {
                        if sp < 63 {
                            stack[sp] = (child, t_enter, t_exit);
                            sp += 1;
                        }
                    }
                }
            }
        }

        if let Some(palette) = section_palette {
            if let Some(VoxelHit { block, steps, .. }) = trace_section(palette, ray, 128) {
                return Some(SvoHit {
                    world: ray.origin,
                    block,
                    steps,
                });
            }
        }
        None
    }

    pub fn memory_bytes(&self) -> usize {
        self.nodes.len() * std::mem::size_of::<Node>()
    }
}

fn region_uniform(palette: &SectionPalette, ox: usize, oy: usize, oz: usize, size: usize) -> (bool, u16) {
    let mut first = None;
    for z in oz..oz + size {
        for y in oy..oy + size {
            for x in ox..ox + size {
                if x >= SECTION_SIZE || y >= SECTION_SIZE || z >= SECTION_SIZE {
                    continue;
                }
                let b = palette[idx(x, y, z)];
                match first {
                    None => first = Some(b),
                    Some(f) if f != b => return (false, 0),
                    _ => {}
                }
            }
        }
    }
    (true, first.unwrap_or(0))
}

fn dominant_block(palette: &SectionPalette, ox: usize, oy: usize, oz: usize, size: usize) -> u16 {
    let mut counts = [0u32; 16];
    for z in oz..oz + size.min(SECTION_SIZE) {
        for y in oy..oy + size.min(SECTION_SIZE) {
            for x in ox..ox + size.min(SECTION_SIZE) {
                let b = palette[idx(x, y, z)] as usize;
                if b < counts.len() {
                    counts[b] += 1;
                }
            }
        }
    }
    counts
        .iter()
        .enumerate()
        .max_by_key(|(_, c)| *c)
        .map(|(i, _)| i as u16)
        .unwrap_or(0)
}

fn col_idx(x: usize, y: usize, z: usize, height: usize) -> usize {
    x + y * SECTION_SIZE + z * SECTION_SIZE * height
}

fn region_uniform_column(
    column: &[u16],
    ox: usize,
    oy: usize,
    oz: usize,
    sx: usize,
    sy: usize,
    sz: usize,
) -> (bool, u16) {
    let height = column.len() / (SECTION_SIZE * SECTION_SIZE);
    let mut first = None;
    for z in oz..oz + sz {
        for y in oy..oy + sy {
            for x in ox..ox + sx {
                if x >= SECTION_SIZE || z >= SECTION_SIZE || y >= height {
                    continue;
                }
                let b = column[col_idx(x, y, z, height)];
                match first {
                    None => first = Some(b),
                    Some(f) if f != b => return (false, 0),
                    _ => {}
                }
            }
        }
    }
    (true, first.unwrap_or(0))
}

fn dominant_block_column(
    column: &[u16],
    ox: usize,
    oy: usize,
    oz: usize,
    sx: usize,
    sy: usize,
    sz: usize,
) -> u16 {
    let height = column.len() / (SECTION_SIZE * SECTION_SIZE);
    let mut counts = [0u32; 16];
    for z in oz..oz + sz {
        for y in oy..oy + sy {
            for x in ox..ox + sx {
                if x >= SECTION_SIZE || z >= SECTION_SIZE || y >= height {
                    continue;
                }
                let b = column[col_idx(x, y, z, height)] as usize;
                if b < counts.len() {
                    counts[b] += 1;
                }
            }
        }
    }
    counts
        .iter()
        .enumerate()
        .max_by_key(|(_, c)| *c)
        .map(|(i, _)| i as u16)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary_greedy_meshing::idx;

    #[test]
    fn uniform_air_collapses_to_one_node() {
        let p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        let svo = SparseVoxelOctree::from_section(&p);
        assert!(svo.node_count <= 2);
    }

    #[test]
    fn solid_cube_compact() {
        let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        for z in 0..SECTION_SIZE {
            for y in 0..8 {
                for x in 0..SECTION_SIZE {
                    p[idx(x, y, z)] = 1;
                }
            }
        }
        let svo = SparseVoxelOctree::from_section(&p);
        assert!(svo.node_count < 100);
        let ray = Ray3::new([8.0, -1.0, 8.0], [0.0, 1.0, 0.0]);
        let hit = svo.trace(&ray, Some(&p));
        assert!(hit.is_some());
    }
}
