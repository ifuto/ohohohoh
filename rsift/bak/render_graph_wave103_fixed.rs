//! Full render graph with resource tracking + topological schedule (Tier 6).
//! Also keeps Feather-compatible `RenderGraphScheduler::new(merged, minimal)`.
//!
//! **依存モデル (wave 103 DC-1)**: forward-hazard 型 — パス宣言順を有効な
//! 実行順と見做し、`i < j` のペアに同一資源の RAW (i 書 → j 読) / WAR
//! (i 読 → j 書) / WAW (i 書 → j 書) の hazard があれば `i → j` の依存を
//! 張る。これにより同一資源を read+write する RMW パス鎖 (translucent や
//! taa の Color) は宣言順に直列化され、グラフは**構築上必ず DAG** になる。
//! 旧実装は RAW のみを全順序ペアで張ったため、`taa write Color → translucent
//! read Color` の逆エッジで `translucent ↔ taa` が**真のサイクル**となり、
//! Kahn が 2 パス (translucent, taa_composite) を schedule から**静寂脱落**
//! させていた (schedule=[0,1] のみ、total_cost=6、barriers=2 のまま弱い
//! assert でテストが素通りしていた = フレームグラフ半消失の潜伏実害)。

use tracing::trace;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RenderPassKind {
    TerrainMerged,
    Translucent,
    Composite,
}

#[derive(Debug, Clone)]
pub struct ScheduledPass {
    pub kind: RenderPassKind,
    pub merge_ao: bool,
    pub merge_water: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GraphResource {
    Color,
    Depth,
    History,
    Velocity,
    Hzb,
    Shadow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceState {
    Undefined,
    RenderTarget,
    DepthWrite,
    ShaderRead,
    CopySrc,
    CopyDst,
}

#[derive(Debug, Clone)]
pub struct GraphPassNode {
    pub name: &'static str,
    pub reads: Vec<(GraphResource, ResourceState)>,
    pub writes: Vec<(GraphResource, ResourceState)>,
    pub cost: u32,
}

/// リソースの**使用状態推移点** (「GPU queue にそのまま発行する外部バリア列」
/// ではなく、schedule 順に資源の最終使用状態が変化する点の列挙):
/// 同一パスが同一資源を read+write する RMW パス (translucent/taa の Color
/// 等) では、当該パス内の「read 用状態 → write 用状態」の進行も 1 要素に
/// なり `after_pass == そのパス自身` になる (wave 103 DC-4 明文化)。
#[derive(Debug, Clone)]
pub struct Barrier {
    pub resource: GraphResource,
    pub from: ResourceState,
    pub to: ResourceState,
    /// 推移が起きるパスの schedule 上 index (ノード index。
    /// RMW パスのパス内進行では自身の index)。
    pub after_pass: usize,
}

#[derive(Debug)]
pub struct RenderGraph {
    pub nodes: Vec<GraphPassNode>,
    pub schedule: Vec<usize>,
    pub parallel_groups: Vec<Vec<usize>>,
    pub barriers: Vec<Barrier>,
}

impl RenderGraph {
    pub fn builder() -> RenderGraphBuilder {
        RenderGraphBuilder { nodes: Vec::new() }
    }

    pub fn total_cost(&self) -> u32 {
        self.schedule
            .iter()
            .map(|&i| self.nodes[i].cost)
            .sum()
    }
}

#[derive(Debug, Default)]
pub struct RenderGraphBuilder {
    nodes: Vec<GraphPassNode>,
}

impl RenderGraphBuilder {
    pub fn add_pass(&mut self, node: GraphPassNode) -> &mut Self {
        self.nodes.push(node);
        self
    }

    pub fn build(self) -> RenderGraph {
        let n = self.nodes.len();
        // Dependency (wave 103 DC-1): forward-hazard — i < j のペアに対し、
        // 同一資源の RAW (writes_i ∩ reads_j) / WAR (reads_i ∩ writes_j) /
        // WAW (writes_i ∩ writes_j) があれば i → j。read-read は依存なし。
        // 宣言順を逆向きに張ることはないため構築上必ず DAG (サイクル非成立)。
        let mut indeg = vec![0u32; n];
        let mut adj = vec![Vec::<usize>::new(); n];
        for i in 0..n {
            let writes_i: Vec<_> = self.nodes[i].writes.iter().map(|(r, _)| *r).collect();
            let reads_i: Vec<_> = self.nodes[i].reads.iter().map(|(r, _)| *r).collect();
            for j in (i + 1)..n {
                let writes_j: Vec<_> = self.nodes[j].writes.iter().map(|(r, _)| *r).collect();
                let reads_j: Vec<_> = self.nodes[j].reads.iter().map(|(r, _)| *r).collect();
                let raw = writes_i.iter().any(|r| reads_j.contains(r));
                let war = reads_i.iter().any(|r| writes_j.contains(r));
                let waw = writes_i.iter().any(|r| writes_j.contains(r));
                if raw || war || waw {
                    adj[i].push(j);
                    indeg[j] += 1;
                }
            }
        }
        // Kahn topological + parallel groups (same indegree wave)
        let mut indeg_work = indeg.clone();
        let mut schedule = Vec::new();
        let mut parallel_groups = Vec::new();
        let mut ready: Vec<usize> = indeg_work
            .iter()
            .enumerate()
            .filter(|(_, d)| **d == 0)
            .map(|(i, _)| i)
            .collect();
        while !ready.is_empty() {
            parallel_groups.push(ready.clone());
            let wave = std::mem::take(&mut ready);
            for i in wave {
                schedule.push(i);
                for &j in &adj[i] {
                    indeg_work[j] -= 1;
                    if indeg_work[j] == 0 {
                        ready.push(j);
                    }
                }
            }
        }
        // wave 103 DC-2: 未スケジュール残留は全ノード喪失の静寂化を意味する
        // (旧実装はサイクル残留ノードを schedule/parallel_groups/barriers/
        // total_cost から無言で落としていた)。forward-hazard では構築上
        // 到達不能だが、防御として fail-loud に検査する。
        assert_eq!(
            schedule.len(),
            n,
            "render_graph: {}/{} passes scheduled — サイクル等で未スケジュールのパスが存在 (静寂脱落遮断)",
            schedule.len(),
            n
        );
        // Barriers between state changes along schedule
        let mut last_state = std::collections::HashMap::<GraphResource, ResourceState>::new();
        let mut barriers = Vec::new();
        for &pi in &schedule {
            let node = &self.nodes[pi];
            for &(res, st) in node.reads.iter().chain(node.writes.iter()) {
                if let Some(prev) = last_state.get(&res).copied() {
                    if prev != st {
                        barriers.push(Barrier {
                            resource: res,
                            from: prev,
                            to: st,
                            after_pass: pi,
                        });
                    }
                }
                last_state.insert(res, st);
            }
        }
        RenderGraph {
            nodes: self.nodes,
            schedule,
            parallel_groups,
            barriers,
        }
    }
}

/// Feather-facing scheduler — builds a full graph under the hood.
pub struct RenderGraphScheduler {
    pub merged_subpasses: bool,
    pub minimal_barriers: bool,
    passes: Vec<ScheduledPass>,
    pub graph: RenderGraph,
}

impl RenderGraphScheduler {
    pub fn new(merged_subpasses: bool, minimal_barriers: bool) -> Self {
        let mut s = Self {
            merged_subpasses,
            minimal_barriers,
            passes: Vec::new(),
            graph: RenderGraph {
                nodes: Vec::new(),
                schedule: Vec::new(),
                parallel_groups: Vec::new(),
                barriers: Vec::new(),
            },
        };
        s.build_frame_graph();
        s
    }

    fn build_frame_graph(&mut self) {
        self.passes.clear();
        if self.merged_subpasses {
            self.passes.push(ScheduledPass {
                kind: RenderPassKind::TerrainMerged,
                merge_ao: true,
                merge_water: true,
            });
        } else {
            self.passes.push(ScheduledPass {
                kind: RenderPassKind::TerrainMerged,
                merge_ao: false,
                merge_water: false,
            });
        }
        self.passes.push(ScheduledPass {
            kind: RenderPassKind::Translucent,
            merge_ao: false,
            merge_water: false,
        });
        self.passes.push(ScheduledPass {
            kind: RenderPassKind::Composite,
            merge_ao: false,
            merge_water: false,
        });

        let mut b = RenderGraph::builder();
        b.add_pass(GraphPassNode {
            name: "depth_hzb",
            reads: vec![],
            writes: vec![
                (GraphResource::Depth, ResourceState::DepthWrite),
                (GraphResource::Hzb, ResourceState::CopyDst),
            ],
            cost: 2,
        });
        b.add_pass(GraphPassNode {
            name: "opaque_terrain",
            reads: vec![
                (GraphResource::Depth, ResourceState::ShaderRead),
                (GraphResource::Hzb, ResourceState::ShaderRead),
            ],
            writes: vec![
                (GraphResource::Color, ResourceState::RenderTarget),
                (GraphResource::Velocity, ResourceState::RenderTarget),
            ],
            cost: if self.merged_subpasses { 4 } else { 5 },
        });
        b.add_pass(GraphPassNode {
            name: "translucent",
            reads: vec![
                (GraphResource::Depth, ResourceState::ShaderRead),
                (GraphResource::Color, ResourceState::ShaderRead),
            ],
            writes: vec![(GraphResource::Color, ResourceState::RenderTarget)],
            cost: 3,
        });
        b.add_pass(GraphPassNode {
            name: "taa_composite",
            reads: vec![
                (GraphResource::Color, ResourceState::ShaderRead),
                (GraphResource::History, ResourceState::ShaderRead),
                (GraphResource::Velocity, ResourceState::ShaderRead),
            ],
            writes: vec![
                (GraphResource::Color, ResourceState::RenderTarget),
                (GraphResource::History, ResourceState::CopyDst),
            ],
            cost: 2,
        });
        self.graph = b.build();
        if self.minimal_barriers {
            // Keep only transitions that change usage class
            // (wave 103 DC-5 注: 推移列の構築規則上 `from != to` は生成時に
            //  既に保証され (prev != st のときだけ push)、同一資源・同一 `to` の
            //  連続も生じ得ない (資源ごとの状態は単一 last_state を交互に遷移
            //  するため、同じ (resource,to) が 2 連続するには間に異なる `to` が
            //  必要) — つまり両フィルタは**構造上到達不能な防御**であり、
            //  実害はないが_fail-safe_ として維持する。)
            self.graph.barriers.retain(|b| b.from != b.to);
            // Collapse duplicate consecutive same-resource barriers
            self.graph.barriers.dedup_by(|a, b| a.resource == b.resource && a.to == b.to);
        }
    }

    /// 実グラフの推移点の本数を**そのまま**返す (wave 103 DC-3 正直化)。
    /// 旧実装は minimal 時に `len.max(1)`・non-minimal 時に `len * 2` の
    /// **虚構メトリクス** (構造的根拠なしの見栄え係数) を返していた。
    /// 消費者は `log_schedule()` の trace ログのみで、値の大小比較・
    /// 進捗管理には使われていないため、実数への置換は安全。
    pub fn barrier_count(&self) -> u32 {
        self.graph.barriers.len() as u32
    }

    /// Feather 宣言パスの scaffold 一覧。`merge_ao` / `merge_water` 等の
    /// フラグに外部消費者は現状いない (render_pipeline は Graph 側のみ使用)。
    /// 将来のサブパス分割 wiring 用に温存 (wave 103 DC-7 観察)。
    pub fn passes(&self) -> &[ScheduledPass] {
        &self.passes
    }

    pub fn log_schedule(&self) {
        trace!(
            "[RenderGraph] feathers={} graph_passes={} barriers={} parallel_waves={}",
            self.passes.len(),
            self.graph.schedule.len(),
            self.barrier_count(),
            self.graph.parallel_groups.len()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topo_and_barriers() {
        // wave 103 DC-1 以降は完全スケジュールが保証される: 4 パス全てが
        // 宣言順に並び、全ノード到達・全コスト集計される (旧実装は真のサイクル
        // で 2 パス静寂脱落していたが本テストの弱い条件では検出できなかった)。
        let s = RenderGraphScheduler::new(true, true);
        assert_eq!(s.graph.schedule, vec![0, 1, 2, 3]);
        assert_eq!(
            s.graph.parallel_groups,
            vec![vec![0], vec![1], vec![2], vec![3]]
        );
        assert_eq!(s.barrier_count(), 8);
        assert_eq!(s.graph.total_cost(), 11);
    }

    mod strict_tests {
        use super::*;

        /// Feather 宣言グラフ (深度+Hzb → 不透明地形 → 半透明 RMW → TAA 合成
        /// RMW) の完全ピン。旧実害 (translucent ↔ taa 真サイクルで schedule
        /// = [0,1] 脱落、total_cost=6、barriers=2) の回帰を全要素で遮断する。
        #[test]
        fn feather_graph_full_schedule_exact() {
            for &(merged, want_cost) in &[(true, 11u32), (false, 12u32)] {
                let s = RenderGraphScheduler::new(merged, false);
                let names: Vec<&str> = s.graph.nodes.iter().map(|n| n.name).collect();
                assert_eq!(
                    names,
                    vec![
                        "depth_hzb",
                        "opaque_terrain",
                        "translucent",
                        "taa_composite"
                    ]
                );
                assert_eq!(s.graph.schedule, vec![0, 1, 2, 3], "merged={merged}");
                assert_eq!(
                    s.graph.parallel_groups,
                    vec![vec![0], vec![1], vec![2], vec![3]],
                    "merged={merged}"
                );
                assert_eq!(s.graph.total_cost(), want_cost, "merged={merged}");
            }
        }

        /// 推移点列の 8 要素を (resource, from, to, after_pass) で完全ピン。
        /// [3] と [6] は RMW パス内の「read 用状態 → write 用状態」進行で
        /// after_pass == パス自身 (DC-4 明文化分)、[7] は閲覧→CopyDst 進行。
        #[test]
        fn feather_barrier_stream_exact() {
            use GraphResource as R;
            use ResourceState as S;
            let s = RenderGraphScheduler::new(true, true);
            let got: Vec<(R, S, S, usize)> = s
                .graph
                .barriers
                .iter()
                .map(|b| (b.resource, b.from, b.to, b.after_pass))
                .collect();
            let want = vec![
                (R::Depth, S::DepthWrite, S::ShaderRead, 1),
                (R::Hzb, S::CopyDst, S::ShaderRead, 1),
                (R::Color, S::RenderTarget, S::ShaderRead, 2),
                (R::Color, S::ShaderRead, S::RenderTarget, 2), // RMW 自己遷移
                (R::Color, S::RenderTarget, S::ShaderRead, 3),
                (R::Velocity, S::RenderTarget, S::ShaderRead, 3),
                (R::Color, S::ShaderRead, S::RenderTarget, 3), // RMW 自己遷移
                (R::History, S::ShaderRead, S::CopyDst, 3),
            ];
            assert_eq!(got, want);
        }

        /// DC-3: 実本数を両モードでそのまま返す (旧 non-minimal = len*2=16 の
        /// 虚構係数の回帰を遮断)。
        #[test]
        fn barrier_count_honest_both_modes() {
            assert_eq!(RenderGraphScheduler::new(true, true).barrier_count(), 8);
            assert_eq!(RenderGraphScheduler::new(true, false).barrier_count(), 8);
            assert_eq!(RenderGraphScheduler::new(false, true).barrier_count(), 8);
            assert_eq!(RenderGraphScheduler::new(false, false).barrier_count(), 8);
        }

        fn chain_node(
            name: &'static str,
            reads: Vec<(GraphResource, ResourceState)>,
            writes: Vec<(GraphResource, ResourceState)>,
        ) -> GraphPassNode {
            GraphPassNode {
                name,
                reads,
                writes,
                cost: 1,
            }
        }

        /// 宣言順 = 有効実行順の RMW 直列化: 相互に Color を read+write する
        /// 2 パスは旧実装では RAW 双方向エッジで真のサイクル (schedule 空)、
        /// forward-hazard では宣言順に [0,1] へ直列化される最小モデル。
        #[test]
        fn declaration_order_serializes_rmw_pair() {
            use GraphResource as R;
            use ResourceState as S;
            let mut b = RenderGraph::builder();
            b.add_pass(chain_node(
                "p0",
                vec![(R::Color, S::ShaderRead)],
                vec![(R::Color, S::RenderTarget)],
            ));
            b.add_pass(chain_node(
                "p1",
                vec![(R::Color, S::ShaderRead)],
                vec![(R::Color, S::RenderTarget)],
            ));
            let g = b.build();
            assert_eq!(g.schedule, vec![0, 1]);
            assert_eq!(g.parallel_groups, vec![vec![0], vec![1]]);
            let got: Vec<(S, S, usize)> = g
                .barriers
                .iter()
                .map(|b| (b.from, b.to, b.after_pass))
                .collect();
            assert_eq!(
                got,
                vec![
                    (S::ShaderRead, S::RenderTarget, 0),
                    (S::RenderTarget, S::ShaderRead, 1),
                    (S::ShaderRead, S::RenderTarget, 1)
                ]
            );
        }

        /// 書-書-読-書の 4 連: WAW(0→1) / RAW(1→2) / WAR(2→3) が全て
        /// 張られることで完全直列グループ [[0],[1],[2],[3]] になる。
        /// WAW のいずれか 1 本でも欠けると [0,1] が同一 wave に崩れる。
        #[test]
        fn declaration_order_serializes_chain_4_all_hazards() {
            use GraphResource as R;
            use ResourceState as S;
            let mut b = RenderGraph::builder();
            b.add_pass(chain_node("w1", vec![], vec![(R::Color, S::RenderTarget)]));
            b.add_pass(chain_node("w2", vec![], vec![(R::Color, S::RenderTarget)]));
            b.add_pass(chain_node("r1", vec![(R::Color, S::ShaderRead)], vec![]));
            b.add_pass(chain_node("w3", vec![], vec![(R::Color, S::RenderTarget)]));
            let g = b.build();
            assert_eq!(g.schedule, vec![0, 1, 2, 3]);
            assert_eq!(g.parallel_groups, vec![vec![0], vec![1], vec![2], vec![3]]);
            let got: Vec<(S, S, usize)> = g
                .barriers
                .iter()
                .map(|b| (b.from, b.to, b.after_pass))
                .collect();
            // 状態推移: RT(w1)→RT(w2) は推移なし、→SR(r1)@2、→RT(w3)@3
            assert_eq!(
                got,
                vec![
                    (S::RenderTarget, S::ShaderRead, 2),
                    (S::ShaderRead, S::RenderTarget, 3)
                ]
            );
        }

        /// 空グラフは合法: スケジュール/グループ/推移が全て空で、
        /// DC-2 の fail-loud assert (schedule.len()==n) も 0==0 で通る。
        #[test]
        fn empty_graph_is_wellformed() {
            let g = RenderGraph::builder().build();
            assert!(g.schedule.is_empty());
            assert!(g.parallel_groups.is_empty());
            assert!(g.barriers.is_empty());
            assert_eq!(g.total_cost(), 0);
        }
    }
}
