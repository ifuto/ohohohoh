//! Sparse Voxel Octree (SVO) — far-LOD / zero-polygon path with branchless DDA leaf refine.

use crate::binary_greedy_meshing::{idx, SectionPalette, SECTIONS_PER_COLUMN, SECTION_SIZE};
use crate::branchless_dda::{trace_section, Ray3, VoxelHit};
use tracing::trace;

const MAX_DEPTH: u32 = 4; // 2^4 = 16 (16^3 セクションキューブ用)

/// 2 冪天井 log2 (1→0, 16→4, 64→6)。カラム octree のキャップに使う。
fn ceil_log2_usize(mut v: usize) -> u32 {
    let mut l = 0u32;
    while v > 1 {
        v = v.div_ceil(2);
        l += 1;
    }
    l
}

/// GPU アップロード用語列の 1 ノードあたりストライド (u32 数)。
/// レイアウト: word0 = 0:Empty / 1:Branch / (2+b):Uniform(block b)、
/// words 1..=8 = 8 子インデックス、word9 = pad。
/// `shaders/voxel_cone_tracing.wgsl` の実走査と一致する。
pub const GPU_NODE_STRIDE: usize = 10;

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
    /// ツリー最大深度 (16³ セクションは 4、16x64x16 カラムは 6)。
    /// `sample_lod` の降下 budget と GPU 走査 (VctParams.cap) が一致するために必須。
    pub max_depth: u32,
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
            bounds: [
                SECTION_SIZE,
                SECTION_SIZE,
                SECTION_SIZE * SECTIONS_PER_COLUMN,
            ],
            node_count: 0,
            leaf_count: 0,
            max_depth: MAX_DEPTH,
        }
    }

    /// Build SVO from one 16³ section palette.
    pub fn from_section(palette: &SectionPalette) -> Self {
        let mut tree = Self::empty();
        tree.bounds = [SECTION_SIZE, SECTION_SIZE, SECTION_SIZE];
        tree.root = tree.build_node(palette, 0, 0, 0, SECTION_SIZE);
        tree.node_count = tree.nodes.len() as u32;
        trace!(
            "[SVO] section built: {} nodes, {} leaves",
            tree.node_count,
            tree.leaf_count
        );
        tree
    }

    /// Stack 1..=4 sections into one column octree (16×H×16, H は 2 冪 pad)。
    ///
    /// 注: ツリーは軸別「正確半分」分割のため論理高さは 2 冪に揃える
    /// (3 セクション=48 のような非 2 冪高さは 64 にゼロパディング。
    /// パディング部は空気として uniform 折り畳みされ、`sample_lod` は
    /// 実高さより上の問い合わせに None=空気 で正しく答える)。
    /// `bounds` はこのパディング後の論理境界を示す。
    ///
    /// **契約 (wave 57 BG 監査で fail-loud 化)**: `sections` は 1..=4 本。
    /// 旧実装は 5 本以上を `min/take` で**静寂切捨て** (実ボクセル消失)、
    /// 0 本を高さ 1 の退化ツリーに**静寂着地**させていた — 両方とも拒否。
    pub fn from_column(sections: &[SectionPalette]) -> Self {
        assert!(
            !sections.is_empty(),
            "from_column 契約違反: sections が 0 本 (空カラムに意味論なし — 高さ 1 退化ツリーへの静寂着地を拒否)"
        );
        assert!(
            sections.len() <= SECTIONS_PER_COLUMN,
            "from_column 契約違反: sections {} 本 > SECTIONS_PER_COLUMN={} (旧來の静寂切捨ては実ボクセル消失のため拒否)",
            sections.len(),
            SECTIONS_PER_COLUMN
        );
        let height = SECTION_SIZE * sections.len();
        let height = height.next_power_of_two();
        let mut column = vec![0u16; SECTION_SIZE * height * SECTION_SIZE];
        for (sy, sec) in sections.iter().enumerate() {
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
        // 最深キャップは最大軸の 2 冪 log2 (16x64x16 → 6)。ツリーは軸別半分割で
        // 最深 1x1x1 葉まで到達する (`build_node_column` 参照)。
        tree.max_depth = ceil_log2_usize(SECTION_SIZE.max(height));
        tree.root = tree.build_node_column(&column, 0, 0, 0, SECTION_SIZE, height, SECTION_SIZE);
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
        // wave 57 BG-2: 旧来の `depth >= MAX_DEPTH || size <= 1` キャップ +
        // dominant 葉は**到達不能** (再起で size は必ず 2 冪半減列 16,8,4,2,1
        // を辿り、size==1 = 1 セルは直前の uniform 判定に恒に捕捉される;
        // depth>=4 ⇒ size==1) であったため撤去。旧 dominant はブロック id ≥16
        // を **カウント対象外として静寂に落とす** 潜在バグを抱えていた
        // (blocks≥16 のみのセルが Empty に化け得た) — 到達不能化を証明付きで
        // 撤去できたことで、そのハザードを構造的に根絶する。終了性は
        // 「非一様 ⇒ size≥2 ⇒ 子は厳密半減」より保証 (最深 1x1x1 voxel 厳密葉)。
        let half = size / 2;
        let mut children = [0u32; 8];
        let mut child_idx = 0usize;
        for dz in 0..2 {
            for dy in 0..2 {
                for dx in 0..2 {
                    let cx = ox + dx * half;
                    let cy = oy + dy * half;
                    let cz = oz + dz * half;
                    children[child_idx] = self.build_node(palette, cx, cy, cz, half.max(1));
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
        // wave 57 BG-2: 旧来の `depth >= max_depth || 全軸<=1` キャップ +
        // dominant 葉は到達不能 (max_depth=ceil_log2(最大軸) で、レベル d で
        // 各軸は max(1, s>>d) に同期半減 → depth=max_depth で恒に 1x1x1、
        // かつ 1x1x1 = 1 セルは直前の uniform 判定に捕捉) のため撤去
        // (blocks≥16 静寂消失ハザードを構造的に根絶)。終了性は
        // 「非一様 ⇒ 2 セル以上 ⇒ いずれかの軸 ≥2 ⇒ 子はその軸で厳密半減」より保証。
        // 軸別半分割 (16x64x16 → 8x32x8 → … → 1x1x1)。
        // 注: 以前は min 軸の cubic 半分割 (8^3 固定) で、カラムでは
        // root 直下から y>=16 の領域がツリーに一切生成されず、
        // `sample_lod` (軸別仮定) とも幾何が不一致だった (Phase D で実修正)。
        // 軸幅 1 の場合はその軸の分割を凍結する。**凍結軸の 2 スロットは
        // 同一サブツリーを共有 (DAG 化)**: 縮退分裂で最悪 ~6.5x に膨張した
        // メモリ (実測 4.87MB/列) を正味ツリー規模へ抑えるための実対策。
        // sampler の dx 選択は半幅比較なので共有しても選択結果は同一。
        let hx = (sx / 2).max(1);
        let hy = (sy / 2).max(1);
        let hz = (sz / 2).max(1);
        let xs = if sx > 1 { 2 } else { 1 };
        let ys = if sy > 1 { 2 } else { 1 };
        let zs = if sz > 1 { 2 } else { 1 };
        // 実 (非縮退) の組み合わせのみ 1 回ずつ構築
        let mut real = [0u32; 8]; // index = dxv + 2*dyv + 4*dzv
        for dzv in 0..zs {
            for dyv in 0..ys {
                for dxv in 0..xs {
                    real[dxv + 2 * dyv + 4 * dzv] = self.build_node_column(
                        column,
                        ox + if sx > 1 { dxv * hx } else { 0 },
                        oy + if sy > 1 { dyv * hy } else { 0 },
                        oz + if sz > 1 { dzv * hz } else { 0 },
                        if sx > 1 { hx } else { sx },
                        if sy > 1 { hy } else { sy },
                        if sz > 1 { hz } else { sz },
                    );
                }
            }
        }
        let mut children = [0u32; 8];
        for dz in 0..2usize {
            for dy in 0..2usize {
                for dx in 0..2usize {
                    let rx = if sx > 1 { dx } else { 0 };
                    let ry = if sy > 1 { dy } else { 0 };
                    let rz = if sz > 1 { dz } else { 0 };
                    children[dx + 2 * dy + 4 * dz] = real[rx + 2 * ry + 4 * rz];
                }
            }
        }
        self.alloc_node(Node {
            kind: NodeKind::Branch,
            children,
        })
    }

    /// Voxel Cone Tracing 用ボクセルサンプリング: 実 SVO ノードを LOD 深度まで下降する。
    /// `lod` はコーン直径の log2 (0 = 葉レベルで最深まで下降、大きいほど浅く打ち切る)。
    /// 到達ノードが Uniform なら実ブロック状態を、Branch で深さ打ち切りなら直下 8 子の
    /// 実占有率を返す。空気・範囲外なら None。
    /// 色はクレート内にブロック状態→albedo の実データが存在しないため中性アルベド
    /// (典型的地形平均の 0.5) とし、遮蔽・占有 (alpha) を実ノードデータで駆動する。
    pub fn sample_lod(&self, x: f32, y: f32, z: f32, lod: u32) -> Option<([f32; 3], f32)> {
        const NEUTRAL_ALBEDO: [f32; 3] = [0.5, 0.5, 0.5];
        if self.nodes.is_empty() {
            return None;
        }
        let b = [
            self.bounds[0] as f32,
            self.bounds[1] as f32,
            self.bounds[2] as f32,
        ];
        if !(x >= 0.0 && y >= 0.0 && z >= 0.0 && x < b[0] && y < b[1] && z < b[2]) {
            return None;
        }
        // 下降してよい残り深度 (太いコーンほど浅く打ち切る)。
        // キャップはツリー実深度に一致させる (カラムは 6)。
        let budget = self.max_depth.saturating_sub(lod.min(self.max_depth));
        let mut node_idx = self.root as usize;
        let mut org = [0.0f32; 3];
        let mut size = b;
        let mut depth = 0u32;
        loop {
            let node = &self.nodes[node_idx];
            match node.kind {
                NodeKind::Empty => return None,
                NodeKind::Uniform(_block) => return Some((NEUTRAL_ALBEDO, 1.0)),
                NodeKind::Branch => {
                    if depth >= budget {
                        // 粗 LOD: 直下 8 子の実占有率を alpha として返す。
                        let solid = node
                            .children
                            .iter()
                            .filter(|&&c| !matches!(self.nodes[c as usize].kind, NodeKind::Empty))
                            .count() as f32;
                        return if solid > 0.0 {
                            Some((NEUTRAL_ALBEDO, solid / 8.0))
                        } else {
                            None
                        };
                    }
                    let half = [size[0] * 0.5, size[1] * 0.5, size[2] * 0.5];
                    let dx = (x >= org[0] + half[0]) as usize;
                    let dy = (y >= org[1] + half[1]) as usize;
                    let dz = (z >= org[2] + half[2]) as usize;
                    // build_node_column と同じ子並び (ci = dz*4 + dy*2 + dx)。
                    let ci = dx + dy * 2 + dz * 4;
                    org[0] += half[0] * dx as f32;
                    org[1] += half[1] * dy as f32;
                    org[2] += half[2] * dz as f32;
                    size = half;
                    node_idx = node.children[ci] as usize;
                    depth += 1;
                }
            }
        }
    }

    /// Octree-guided exact ray trace (最近接ヒット、数学的に厳密)。
    ///
    /// **wave 57 BG-1 (critical) で根治**: 旧実装は子ボックスに ray-AABB 交差
    /// 判定が全く無く (全 8 子へ親の t 区間を無条件伝播)、`world` は
    /// `ray.origin` 固定 (t_enter が常に 0)、`steps` は 0 固定、DFS index 順の
    /// 最初の占有葉を返す (ray と交差しなくても・最近接でなくても) 意味論
    /// スタブだった。
    ///
    /// 現実装の保証:
    /// - 各子ボックスに slab 法で交差判定し、交差した子のみを t_enter 昇順で
    ///   走査 (兄弟ボックスは互いに素で子孫の区間は親区間に包含 → DFS 昇順が
    ///   厳密に成立) するため、返る葉は ray に交差する占有葉のうち**最近接**。
    /// - 葉ボックスは最深 1×1×1 の voxel 厳密葉 (非一様領域の折畳み無し)。
    /// - `world` = origin + dir × max(t_enter, 0) = 葉ボックス入射点
    ///   (origin が葉内部なら origin 自身)。`steps` = 訪問ノード数。
    /// - いずれのボックスとも交差しない ray / 幾何から遠ざかる ray → None。
    /// - ray の origin/dir に非有限値 (NaN/±∞) → None (拒否が責務)。
    ///
    /// `section_palette` 契約: ツリーと palette が乖離している場合の救済として、
    /// octree 非ヒット時に限り branchless DDA をフォールバック実行 (葉は voxel
    /// 厳密なので整合時に DDA は不要)。その場合の world もヒットボクセル
    /// [x,x+1)³ への slab 入射 t から厳密復元する (旧実装の ray.origin 嘘を根治)。
    pub fn trace(&self, ray: &Ray3, section_palette: Option<&SectionPalette>) -> Option<SvoHit> {
        let o = ray.origin;
        let d = ray.dir;
        // 非有限値は拒否 (NaN=観測欠測、±∞ 方向は未定義 — crate 哲学統一)。
        if !(o.iter().all(|v| v.is_finite()) && d.iter().all(|v| v.is_finite())) {
            return None;
        }
        if self.nodes.is_empty() {
            return None;
        }
        // dir 成分 0 → inv=±∞。slab 内で 0×∞=NaN は「境界面一致×平行」のみ
        // に発生し、全区間受理として正規化する (slab_hit 参照)。
        let inv = [1.0 / d[0], 1.0 / d[1], 1.0 / d[2]];
        let bounds = [
            self.bounds[0] as f32,
            self.bounds[1] as f32,
            self.bounds[2] as f32,
        ];
        let mut steps = 0u32;
        // 根ボックス非交差なら octree 内部にヒットは存在しない → fallback へ。
        if let Some((t_root, _)) = slab_hit([0.0, 0.0, 0.0], bounds, o, inv) {
            // スタック watermark ≤ 1 + 7×(max_depth) (pop 1/push ≤8、兄弟は
            // 互いに素で子孫区間は親区間に包含)。16x64x16 の最深 6 → ≤43 < 64。
            let (mut stack, mut sp) = ([(0u32, [0.0f32; 3], [0.0f32; 3], 0.0f32); 64], 1usize);
            stack[0] = (self.root, [0.0, 0.0, 0.0], bounds, t_root);
            while sp > 0 {
                sp -= 1;
                let (node_id, lo, size, t_enter) = stack[sp];
                let Some(node) = self.nodes.get(node_id as usize) else {
                    continue; // 防御: 正常ツリーでは到達しない
                };
                steps += 1;
                match node.kind {
                    NodeKind::Empty => {}
                    NodeKind::Uniform(block) => {
                        let t = t_enter.max(0.0);
                        return Some(SvoHit {
                            world: [o[0] + d[0] * t, o[1] + d[1] * t, o[2] + d[2] * t],
                            block,
                            steps,
                        });
                    }
                    NodeKind::Branch => {
                        // ray 交差子のみ収集。凍結軸 (size=1) は両スロットが同一
                        // ノード・同一ボックスに縮退する (build_node_column の DAG
                        // 共有) ためノード id で重複除去。
                        let mut cand = [(0u32, [0.0f32; 3], [0.0f32; 3], 0.0f32); 8];
                        let mut n = 0usize;
                        for ci in 0..8usize {
                            let child = node.children[ci];
                            if cand[..n].iter().any(|&(id, _, _, _)| id == child) {
                                continue;
                            }
                            let (dx, dy, dz) = (ci & 1, (ci >> 1) & 1, (ci >> 2) & 1);
                            let (mut clo, mut csize) = (lo, size);
                            for a in 0..3 {
                                if size[a] > 1.0 {
                                    let h = size[a] * 0.5;
                                    clo[a] += h * [dx, dy, dz][a] as f32;
                                    csize[a] = h;
                                }
                            }
                            if let Some((te, _)) = slab_hit(clo, csize, o, inv) {
                                cand[n] = (child, clo, csize, te);
                                n += 1;
                            }
                        }
                        // t_enter 昇順 (有限/±∞ のみ = 全順序。NaN は slab_hit が
                        // 正規化済みで到達不能)。降順 push で最小 t が先に pop。
                        cand[..n].sort_by(|a, b| {
                            a.3.partial_cmp(&b.3)
                                .expect("slab_hit は t_enter の NaN を生成しない (有限/±∞ のみ)")
                        });
                        for &c in cand[..n].iter().rev() {
                            assert!(
                                sp < 64,
                                "SVO 走査スタック境界違反: watermark ≤ 1+7×max_depth (≤43) のはず"
                            );
                            stack[sp] = c;
                            sp += 1;
                        }
                    }
                }
            }
        }

        // Fallback: ツリーと palette の乖離救済 (旧来契約の枠組みは維持)。
        // DDA ヒットのボクセル入射 t を slab で厳密復元する。
        if let Some(palette) = section_palette {
            if let Some(VoxelHit {
                x,
                y,
                z,
                block,
                steps: dda_steps,
            }) = trace_section(palette, ray, 128)
            {
                let (te, _) = slab_hit([x as f32, y as f32, z as f32], [1.0, 1.0, 1.0], o, inv)
                    .expect("DDA が通過したボクセルは ray 直線と交差する — slab は必ず Some");
                let t = te.max(0.0);
                return Some(SvoHit {
                    world: [o[0] + d[0] * t, o[1] + d[1] * t, o[2] + d[2] * t],
                    block,
                    steps: steps + dda_steps,
                });
            }
        }
        None
    }

    pub fn memory_bytes(&self) -> usize {
        self.nodes.len() * std::mem::size_of::<Node>()
    }

    /// GPU アップロード用の語列化 (`GPU_NODE_STRIDE` = 10 u32 / node)。
    /// `shaders/voxel_cone_tracing.wgsl` の実走査と一致するレイアウト:
    /// word0 = 0:Empty / 1:Branch / (2+b):Uniform(block b)、
    /// words 1..=8 = 8 子インデックス (build_* の子並び = dz*4+dy*2+dx)、word9 = pad。
    pub fn to_gpu_words(&self) -> Vec<u32> {
        let mut w = Vec::with_capacity(self.nodes.len() * GPU_NODE_STRIDE);
        for n in &self.nodes {
            let tag = match n.kind {
                NodeKind::Empty => 0u32,
                NodeKind::Branch => 1u32,
                NodeKind::Uniform(b) => 2u32 + b as u32,
            };
            w.push(tag);
            w.extend_from_slice(&n.children);
            w.push(0);
        }
        w
    }
}

fn region_uniform(
    palette: &SectionPalette,
    ox: usize,
    oy: usize,
    oz: usize,
    size: usize,
) -> (bool, u16) {
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

/// 軸平行ボックス [lo, lo+size) と半直線 origin + t·dir (t ≥ 0) の交差判定
/// (slab 法)。返り値は (t_enter, t_exit) で t_enter は負可 (origin が内部)。
///
/// 数値規約 (wave 57 BG-1):
/// - 呼出側は origin/dir を有限に保証済み (trace が入口で拒否)。
/// - inv=±∞ の軸 (dir 成分 ±0) で (lo−o)==0 または (lo+size−o)==0 のとき
///   0×∞=NaN が発生し得るが、これは「ray がその軸に平行で境界面上に乗る」
///   場合に**限り**発生する (有限×±∞ は ±∞、有限差は有限 → NaN になり得ない)。
///   その軸は全区間受理 ((-∞,+∞)) に正規化する — 幾何学的には面に乗る平行
///   ray は他軸が交差すれば面接触ヒット、という確定規約。ゆえに戻り値 t に
///   NaN は出ない (有限/±∞ のみ → 呼出側のソートは全順序)。
/// - t_exit < 0 (ボックスが ray 背面) または t_enter > t_exit は非交差 None。
fn slab_hit(lo: [f32; 3], size: [f32; 3], origin: [f32; 3], inv: [f32; 3]) -> Option<(f32, f32)> {
    let mut t_enter = f32::NEG_INFINITY;
    let mut t_exit = f32::INFINITY;
    for a in 0..3 {
        let t1 = (lo[a] - origin[a]) * inv[a];
        let t2 = (lo[a] + size[a] - origin[a]) * inv[a];
        let (tmin, tmax) = if t1.is_nan() || t2.is_nan() {
            (f32::NEG_INFINITY, f32::INFINITY) // 面平行・境界一致 (上記規約)
        } else if t1 <= t2 {
            (t1, t2)
        } else {
            (t2, t1)
        };
        t_enter = t_enter.max(tmin);
        t_exit = t_exit.min(tmax);
    }
    if t_enter <= t_exit && t_exit >= 0.0 {
        Some((t_enter, t_exit))
    } else {
        None
    }
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

    /// 回帰 (Phase D で実修正): 旧 builder は min 軸 cubic 半分割で、
    /// 16x64x16 カラムの y>=16 領域がツリーに一切到達しなかった。
    /// 軸別半分割化後は y=20 の単一 voxel 占有を sample_lod が観測できる。
    #[test]
    fn column_svo_covers_full_height() {
        let mut col = [[0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE]; 4];
        col[1][idx(5, 4, 7)] = 1; // world (5, 20, 7)
        let svo = SparseVoxelOctree::from_column(&col);
        assert_eq!(svo.bounds, [16, 64, 16]);
        assert_eq!(svo.max_depth, 6, "16x64x16 → ceil_log2(64) = 6");
        let hit = svo.sample_lod(5.5, 20.5, 7.5, 0);
        assert!(
            matches!(hit, Some((_, a)) if (a - 1.0).abs() < 1e-6),
            "y=20 の占有が最深 LOD で観測できること: {hit:?}"
        );
        assert!(
            svo.sample_lod(5.5, 40.5, 7.5, 0).is_none(),
            "空気域 (y=40) は None"
        );
    }

    /// 葉解像度が 1x1x1 voxel で正確 (1x4x1 粗葉 + dominant による
    /// 空気多数での占有消失が無い) こと: 占有 voxel の直上は空気。
    #[test]
    fn column_leaf_is_exact_voxel() {
        let mut col = [[0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE]; 4];
        col[1][idx(5, 4, 7)] = 1; // world (5, 20, 7)
        let svo = SparseVoxelOctree::from_column(&col);
        assert!(
            svo.sample_lod(5.5, 21.5, 7.5, 0).is_none(),
            "占有 voxel 直上 (5,21,7) は空気のはず — 粗い葉で潰れていないこと"
        );
    }

    /// GPU 語列化レイアウト: ストライド / kind tag / 子並び (dz*4+dy*2+dx) の一致性。
    #[test]
    fn gpu_words_layout_matches_tree() {
        let mut col = [[0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE]; 4];
        col[1][idx(5, 4, 7)] = 1;
        col[0][idx(0, 0, 0)] = 3;
        let svo = SparseVoxelOctree::from_column(&col);
        let words = svo.to_gpu_words();
        assert_eq!(words.len(), svo.nodes.len() * GPU_NODE_STRIDE);
        let root = svo.root as usize;
        // root は Branch (2 箇所に占有) かつ全子参照が nodes と一致
        assert_eq!(words[root * GPU_NODE_STRIDE], 1, "root kind = Branch");
        for i in 0..8 {
            assert_eq!(
                words[root * GPU_NODE_STRIDE + 1 + i],
                svo.nodes[root].children[i],
                "child[{i}] (dz*4+dy*2+dx 並び) が語列と一致"
            );
        }
        // Uniform(3) の葉が tag = 2+3 で存在
        assert!(
            words.chunks(GPU_NODE_STRIDE).any(|c| c[0] == 2 + 3),
            "Uniform(3) 葉の tag 規則 (2+b)"
        );
    }

    /// wave 57 BG-1: 最近接ヒット保証 + 入射点 world の厳密ピン。
    /// 幾何: y∈[0,4) 全体が block 1、y∈[8,12) 全体が block 2 の 2 層。
    /// ボクセル単位でなく折畳まれた uniform 葉でも、葉ボックス入射 t は厳密。
    #[test]
    fn trace_nearest_leaf_and_exact_entry_point() {
        let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        for z in 0..SECTION_SIZE {
            for y in 0..SECTION_SIZE {
                for x in 0..SECTION_SIZE {
                    if y < 4 {
                        p[idx(x, y, z)] = 1;
                    } else if (8..12).contains(&y) {
                        p[idx(x, y, z)] = 2;
                    }
                }
            }
        }
        let svo = SparseVoxelOctree::from_section(&p);
        // 下から +Y: 近い方 (block 1, 葉 y∈[0,4)) にヒット、入射 y=0 厳密。
        let hit = svo
            .trace(&Ray3::new([8.5, -1.0, 8.5], [0.0, 1.0, 0.0]), None)
            .expect("ray は占有葉に交差する");
        assert_eq!(hit.block, 1, "最近接葉 (遠い block 2 ではない)");
        assert_eq!(
            hit.world,
            [8.5, 0.0, 8.5],
            "入射点は厳密に復元 (旧実装の ray.origin 嘘ではない)"
        );
        assert!(
            hit.steps >= 1 && hit.steps <= svo.node_count,
            "steps は訪問ノード数"
        );
        // 空気層 (y=6) から +Y: 背後の block 1 は t_exit<0 で棄却、block 2 に y=8 で入射。
        let hit2 = svo
            .trace(&Ray3::new([8.5, 6.0, 8.5], [0.0, 1.0, 0.0]), None)
            .expect("block 2 葉に交差");
        assert_eq!(hit2.block, 2, "背後の block 1 を飛ばして最近接の block 2");
        assert_eq!(hit2.world, [8.5, 8.0, 8.5]);
    }

    /// wave 57 BG-1: 幾何から遠ざかる ray / ボックス非交差 ray は None
    /// (旧実装は全子無条件伝播で占有葉を拾い Some を返し得た)。
    #[test]
    fn trace_miss_returns_none() {
        let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        for z in 0..SECTION_SIZE {
            for y in 0..8 {
                for x in 0..SECTION_SIZE {
                    p[idx(x, y, z)] = 1;
                }
            }
        }
        let svo = SparseVoxelOctree::from_section(&p);
        // 占有域 (y<8) の上空からさらに +Y へ遠ざかる → None
        assert!(svo
            .trace(&Ray3::new([8.5, 20.0, 8.5], [0.0, 1.0, 0.0]), None)
            .is_none());
        // 完全にボックス外を通る平行 ray (y=100) → None
        assert!(svo
            .trace(&Ray3::new([-1.0, 100.0, 2.5], [1.0, 0.0, 0.0]), None)
            .is_none());
        // グリッド内原点だが空気しか通らない ray (y∈[8,16) の空気柱を +Y) → None
        assert!(
            svo.trace(&Ray3::new([8.5, 8.5, 8.5], [0.0, 1.0, 0.0]), None)
                .is_none(),
            "空気柱に占有葉は無い — 旧 DFS 実装なら他列の占有葉に化け得た"
        );
    }

    /// wave 57 BG-1: dir 成分 0 (inv=±∞) の退化軸でも正しく slab 交差する。
    #[test]
    fn trace_parallel_axis_degenerate_ray() {
        let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        for z in 0..SECTION_SIZE {
            for y in 0..4 {
                for x in 0..SECTION_SIZE {
                    p[idx(x, y, z)] = 3;
                }
            }
        }
        let svo = SparseVoxelOctree::from_section(&p);
        // +X 平行 ray (y=2.5, z=2.5 は占有層内) → x=0 の面で入射 (厳密)
        let hit = svo
            .trace(&Ray3::new([-1.0, 2.5, 2.5], [1.0, 0.0, 0.0]), None)
            .expect("平行 ray は占有層を貫通する");
        assert_eq!(hit.block, 3);
        assert_eq!(
            hit.world,
            [0.0, 2.5, 2.5],
            "x=0 入射面 (±∞ 処理で NaN 不発)"
        );
    }

    /// wave 57 BG-1: palette fallback は入射 t を厳密復元する
    /// (旧実装は world=ray.origin の嘘)。ツリーと palette の乖離救済経路。
    #[test]
    fn trace_palette_fallback_restores_voxel_entry() {
        let air = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        let tree = SparseVoxelOctree::from_section(&air); // root Empty 1 ノード
        let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        p[idx(3, 3, 3)] = 7;
        let hit = tree
            .trace(&Ray3::new([3.5, 3.5, 0.5], [0.0, 0.0, 1.0]), Some(&p))
            .expect("DDA fallback が (3,3,3) にヒット");
        assert_eq!(hit.block, 7);
        assert_eq!(
            hit.world,
            [3.5, 3.5, 3.0],
            "ボクセル [3,3,3]×前側面 z=3 への入射 — 旧実装の ray.origin 固定ではない"
        );
        // steps = octree 訪問 1 (root Empty) + DDA 3 ステップ ((3,3,0..2) → (3,3,3))
        assert_eq!(hit.steps, 4);
    }

    /// wave 57 BG-1: 非有限 ray は拒否 (NaN=観測欠測は drop、±∞ 方向は未定義)。
    #[test]
    fn trace_rejects_non_finite_rays() {
        let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        p[idx(1, 1, 1)] = 1;
        let svo = SparseVoxelOctree::from_section(&p);
        assert!(svo
            .trace(&Ray3::new([f32::NAN, 0.0, 0.0], [0.0, 1.0, 0.0]), None)
            .is_none());
        assert!(svo
            .trace(&Ray3::new([0.0, 0.0, 0.0], [f32::NAN, 0.0, 0.0]), None)
            .is_none());
        assert!(svo
            .trace(&Ray3::new([0.0, 0.0, 0.0], [f32::INFINITY, 0.0, 0.0]), None)
            .is_none());
    }

    /// wave 57 BG-2: dominant 撤去後の voxel 厳密性の性質テスト。
    /// LCG でブロック id ≥16 (=100) を含む混合セクションを生成し、
    /// 全 4096 ボクセル中心で sample_lod ↔ palette が一致すること、
    /// GPU 語列に高 id の tag (2+100) が正しく現れることをピン
    /// (旧 dominant の「id≥16 静寂消失」ハザードは構造的に根絶済み)。
    #[test]
    fn voxel_exact_tree_with_high_block_ids() {
        let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut has_high = false;
        for v in p.iter_mut() {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *v = match (state >> 33) % 4 {
                0 => 0,
                1 => 1,
                2 => 2,
                _ => 100,
            };
            has_high |= *v == 100;
        }
        assert!(has_high, "LCG が block 100 を生成 (性質テストの前提)");
        let svo = SparseVoxelOctree::from_section(&p);
        for z in 0..SECTION_SIZE {
            for y in 0..SECTION_SIZE {
                for x in 0..SECTION_SIZE {
                    let b = p[idx(x, y, z)];
                    let s = svo.sample_lod(x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5, 0);
                    if b == 0 {
                        assert!(s.is_none(), "({x},{y},{z}) は空気 → None (voxel 厳密葉)");
                    } else {
                        assert!(
                            matches!(s, Some((_, a)) if a == 1.0),
                            "({x},{y},{z}) block {b} → Uniform 葉 alpha 1.0 (dominant 消失なし)"
                        );
                    }
                }
            }
        }
        let words = svo.to_gpu_words();
        assert!(
            words.chunks(GPU_NODE_STRIDE).any(|c| c[0] == 2 + 100),
            "Uniform(100) 葉の tag (2+b) が語列に保持 (id≥16 の消失なし)"
        );
    }

    /// wave 57 BG-2 (契約 fail-loud): 空セクション列は拒否。
    #[test]
    #[should_panic(expected = "from_column 契約違反: sections が 0 本")]
    fn from_column_rejects_empty_sections() {
        let _ = SparseVoxelOctree::from_column(&[]);
    }

    /// wave 57 BG-2 (契約 fail-loud): 5 本以上の静寂切捨ては拒否。
    #[test]
    #[should_panic(expected = "静寂切捨ては実ボクセル消失")]
    fn from_column_rejects_overlength_sections() {
        let p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        let _ = SparseVoxelOctree::from_column(&[p; SECTIONS_PER_COLUMN + 1]);
    }

    /// wave 57 BG-3: voxel_cone_tracing.wgsl の SVO 走査語彙が
    /// Rust 側 (to_gpu_words / sample_lod) と一致することの表記ピン。
    #[test]
    fn wgsl_svo_traversal_mirrors_rust_vocabulary() {
        let wgsl = crate::frame_vct::VCT_WGSL;
        assert_eq!(
            GPU_NODE_STRIDE, 10,
            "語列ストライド (WGSL NODE_STRIDE=10u と一致)"
        );
        for needle in [
            "const NODE_STRIDE: u32 = 10u;",
            "// CPU: SparseVoxelOctree::sample_lod と同一規則。",
            "if (w0 == 0u) {",                         // Empty tag 0
            "if (w0 >= 2u) {",                         // Uniform tag = 2+b
            "let ci = u32(dx + dy * 2.0 + dz * 4.0);", // 子並び dz*4+dy*2+dx
            "f32(solid) / 8.0",                        // 粗 LOD 実占有率
        ] {
            assert!(wgsl.contains(needle), "WGSL SVO 語彙ピン乖離: {needle}");
        }
    }
}
