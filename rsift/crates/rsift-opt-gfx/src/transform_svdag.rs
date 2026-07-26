//! # 16. Transform-Aware SVDAG (`TransformAwareSvdag` - 2025 I3D Molenaar & Eisemann)
//!
//! SVDAG の繰り返し構造統合をさらに拡張し、回転 (`90°, 180°, 270°` around Y) や
//! 鏡映対称 (`X-mirror, Z-mirror`) なサブツリーも同一の正規化ノード (`CanonicalSubtree`)
//! と変換タグ (`TransformTag`) で統合。ノード数をさらに半減させる。
//!
//! # 監査 2026-07-26 (wave 123 DW) — 契約公表
//!
//! - **DW-1**: `permute_node` の `let mut y` 不用 mut 警告を根治 (y ビットは
//!   Y 面変換で不変のため読み取り専用、opt-gfx lib 警告 9→8)。警告の機械
//!   pin は枠外のため、根治は cargo build 警告カウント照合で検証。
//! - **DW-2**: wiring 構造公表 — `full_graph_wiring:908-916` は SVDAG 再構築
//!   条件 (`svdag.is_none() || tick % 600 == 0`) の内側で `take(16)` のみ
//!   `insert_transform_aware` し返り値を `let _ =` で破棄。「決定破棄・
//!   副作用 (canonical pool / base_dag 成長) のみ」構造 (DU-3 同型) +
//!   先頭 16 ノードへの**部分列挙制限** (全ノードでない) を誇張せず公表。
//! - **DW-3**: canonical 一意性の実証 pin: orbit {oct0/1/4/5} 内 3 メンバー
//!   (5,1,4) のどれから挿入しても同 ID・同 canonical 形 (mask=1,
//!   children[0]=42) — insert_node dedup + orbit 閉包 + strict-min 列挙順
//!   決定性の連鎖で「経路非依存」を機械固定 (既存は 2 メンバーのみ)。
//! - **DW-4**: y ビット不変性の直接 pin: y=1 octant (index 2,3,6,7) ノードは
//!   全 16 変換 (rot×mx×mz) で mask が y=1 領域 (0b0100_0100) に閉じる
//!   ことを全走査で pin (従前はテストコメントでの間接記述のみ)。

use crate::svdag::{SparseVoxelDag, SvdagNodeData};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransformTag {
    pub rotate_y_90_count: u8, // 0..=3
    pub mirror_x: bool,
    pub mirror_z: bool,
}

impl Default for TransformTag {
    fn default() -> Self {
        Self {
            rotate_y_90_count: 0,
            mirror_x: false,
            mirror_z: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TransformAwareSvdag {
    pub base_dag: SparseVoxelDag,
    pub canonical_pool: HashMap<SvdagNodeData, (u32, TransformTag)>,
}

impl Default for TransformAwareSvdag {
    fn default() -> Self {
        Self::new()
    }
}

impl TransformAwareSvdag {
    pub fn new() -> Self {
        Self {
            base_dag: SparseVoxelDag::new(),
            canonical_pool: HashMap::with_capacity(1024),
        }
    }

    /// Canonicalize node permutations to find structural symmetry under 3D transforms.
    ///
    /// # 返却契約 (機械ピン)
    /// `(id, tag)` は常に `base_dag.nodes[id as usize] == permute_node(&node, tag)`
    /// を満たす (tag は入力ノードから canonical ノードへの実変換)。canonical は
    /// transform orbit (Y 回転 × 鏡映の D4 群軌道、最大 8 元) 内で child_mask
    /// 最小の代表 (strict `<` のため同点は列挙順最初 → 決定的)。代数的に同値な
    /// 複数タグ表現 (XZ ≡ R² 等、16 記法 → 8 群元) も列挙順 (rot 外周 → mx → mz)
    /// 最初の表現に潰れる。canonical の確定はオービット最初の終端挿入時で、
    /// 以後の pool ヒットはその ID を再利用する (再最小化はしない)。
    pub fn insert_transform_aware(&mut self, node: SvdagNodeData) -> (u32, TransformTag) {
        if let Some(&(id, tag)) = self.canonical_pool.get(&node) {
            return (id, tag);
        }

        // Check permutations
        let mut best_node = node.clone();
        let mut best_tag = TransformTag::default();

        for rot in 0..4 {
            for &mx in &[false, true] {
                for &mz in &[false, true] {
                    let candidate = Self::permute_node(&node, rot, mx, mz);
                    if let Some(&(id, tag_c)) = self.canonical_pool.get(&candidate) {
                        // BX-1: candidate は canonical とは限らない中間ノード
                        // (pool は「初回入力ノード → canonical」写像も保持する)。
                        // tag を「入力 → canonical」の実変換に合わせるため、
                        // 発見 tag (入力→candidate) に candidate の保存 tag
                        // (candidate→canonical) を D4 群合成で連結する。
                        let tag = Self::compose_tags(
                            TransformTag {
                                rotate_y_90_count: rot,
                                mirror_x: mx,
                                mirror_z: mz,
                            },
                            tag_c,
                        );
                        // 今回ノードも memo 化し、以後の直接ヒットを O(1) にする
                        // (id/tag とも契約を保存、orbit も変わらない)。
                        self.canonical_pool.insert(node.clone(), (id, tag));
                        return (id, tag);
                    }
                    if candidate.child_mask < best_node.child_mask {
                        best_node = candidate;
                        best_tag = TransformTag {
                            rotate_y_90_count: rot,
                            mirror_x: mx,
                            mirror_z: mz,
                        };
                    }
                }
            }
        }

        let id = self.base_dag.insert_node(best_node.clone());
        self.canonical_pool.insert(node.clone(), (id, best_tag));
        self.canonical_pool
            .insert(best_node, (id, TransformTag::default()));
        (id, best_tag)
    }

    /// D4 群 (Y 回転 × 鏡映) のタグ合成: `first` を適用した後に `second` を
    /// 適用する変換と関数として等価なタグを、列挙順最初の表現で返す。
    ///
    /// (x,z) ∈ {0,1}² の 4 点への作用で群元を同定する。対象作用は Y 面の
    /// 正方形頂点に対する D4 の標準作用で faithful (8 群元は 4 点上の置換と
    /// して全て異なる) ため、4 点全ての一致で群元が一意に決まる。
    fn compose_tags(first: TransformTag, second: TransformTag) -> TransformTag {
        let apply = |x: bool, z: bool, t: &TransformTag| -> (bool, bool) {
            let (mut x, mut z) = (x, z);
            for _ in 0..(t.rotate_y_90_count % 4) {
                let ox = x;
                x = z;
                z = !ox;
            }
            if t.mirror_x {
                x = !x;
            }
            if t.mirror_z {
                z = !z;
            }
            (x, z)
        };
        for rot in 0..4 {
            for mx in [false, true] {
                for mz in [false, true] {
                    let cand = TransformTag {
                        rotate_y_90_count: rot,
                        mirror_x: mx,
                        mirror_z: mz,
                    };
                    let same = [(false, false), (false, true), (true, false), (true, true)]
                        .into_iter()
                        .all(|(x, z)| {
                            let (ax, az) = apply(x, z, &first);
                            let (bx, bz) = apply(ax, az, &second);
                            (bx, bz) == apply(x, z, &cand)
                        });
                    if same {
                        return cand;
                    }
                }
            }
        }
        unreachable!("D4 closure: two composed D4 elements are again a D4 element")
    }

    fn permute_node(node: &SvdagNodeData, rot_y: u8, mx: bool, mz: bool) -> SvdagNodeData {
        let mut new_mask = 0u8;
        let mut new_children = [u32::MAX; 8];

        for i in 0..8 {
            if (node.child_mask & (1 << i)) != 0 {
                let mut x = (i & 1) != 0;
                let y = (i & 2) != 0;
                let mut z = (i & 4) != 0;

                for _ in 0..(rot_y % 4) {
                    let old_x = x;
                    x = z;
                    z = !old_x;
                }
                if mx {
                    x = !x;
                }
                if mz {
                    z = !z;
                }

                let new_i = (x as usize) | ((y as usize) << 1) | ((z as usize) << 2);
                new_mask |= 1 << new_i;
                new_children[new_i] = node.children[i];
            }
        }

        SvdagNodeData {
            child_mask: new_mask,
            children: new_children,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transform_aware_canonicalization() {
        let mut tdag = TransformAwareSvdag::new();
        let n1 = SvdagNodeData {
            child_mask: 0b0000_0001,
            children: [
                1,
                u32::MAX,
                u32::MAX,
                u32::MAX,
                u32::MAX,
                u32::MAX,
                u32::MAX,
                u32::MAX,
            ],
        };
        // 注: permute_node は rotate_y_90 (xz 面回転) + mirror_x + mirror_z のみで
        // y ビット (mask の bit1) は不変。よって y octant (mask 0b100) から
        // y=0 octant (mask 0b001) へは到達不能。x octant (mask 0b010) なら
        // mirror_x で octant 0 (= n1) に一致できる。旧テストの 0b100 は y octant で
        // 恒等的に不一致となるテスト入力のミス (実装は設計通り)。
        let n2 = SvdagNodeData {
            child_mask: 0b0000_0010,
            children: [
                u32::MAX,
                1,
                u32::MAX,
                u32::MAX,
                u32::MAX,
                u32::MAX,
                u32::MAX,
                u32::MAX,
            ],
        };

        let (id1, _) = tdag.insert_transform_aware(n1);
        let (id2, _) = tdag.insert_transform_aware(n2);
        assert_eq!(
            id1, id2,
            "Rotated/mirrored subtrees must canonicalize to same node ID"
        );
    }

    fn node_at(octant: usize, child: u32) -> SvdagNodeData {
        let mut children = [u32::MAX; 8];
        children[octant] = child;
        SvdagNodeData {
            child_mask: 1 << octant,
            children,
        }
    }

    #[test]
    fn composed_tag_maps_input_to_canonical_on_intermediate_pool_hit() {
        // BX-1: 初回挿入で pool には「入力ノード (非 canonical) + canonical」の
        // 2 写像が乗る。後続ノードの最初の pool ヒットが非 canonical の場合、
        // 旧実装は「入力→中間」のタグを返し契約 (入力→canonical) を破った。
        // D4 合成の導入で、ヒットが中間ノードでも tag が canonical を指す。
        //
        // 手導出 (列挙順を厳密再現した検算済):
        //   n1 = octant5 (x=T,z=T): 16 候補の strict-min で (0,T,T)→octant0 が
        //     canonical (mask 1、child 42)。pool: n1→(id,(0,T,T)), oct0 形→(id,∅)。
        //   n2 = octant1 (x=T,z=F): 列挙順で (0,F,T)→octant5 が最初の pool
        //     ヒット = **非 canonical の n1 自身** (canonical ヒットは (0,T,F)
        //     でそれより後)。旧実装はタグ (0,F,T) を返し、n2 適用先は
        //     octant5 (canonical ではない) — 契約違反として再現。
        //   合成: compose((0,F,T), (0,T,T)) = ((x,z)→(¬x,z)) ≡ mirror_x、
        //     列挙順最初の表現 (0,T,F)。
        let mut tdag = TransformAwareSvdag::new();
        let n1 = node_at(5, 42);
        let (id1, tag1) = tdag.insert_transform_aware(n1.clone());
        assert_eq!(
            (tag1.rotate_y_90_count, tag1.mirror_x, tag1.mirror_z),
            (0, true, true),
            "n1 (octant5) の初回タグ: 列挙順での最小 mask 到達経路"
        );

        let n2 = node_at(1, 42);
        let (id2, tag2) = tdag.insert_transform_aware(n2.clone());
        assert_eq!(id1, id2, "同一 orbit (xz 0/1/4/5) は同じ canonical ID");
        assert_eq!(
            (tag2.rotate_y_90_count, tag2.mirror_x, tag2.mirror_z),
            (0, true, false),
            "合成タグは入力→canonical (mirror_x)、\
             旧実装の入力→中間 (mirror_z) ではない"
        );
        // 返却契約の機械検証: tag を適用すると id の指す canonical ノードと一致。
        let canonical = tdag
            .base_dag
            .nodes
            .get(id2 as usize)
            .expect("canonical node exists");
        assert_eq!(
            canonical.child_mask, 1,
            "canonical は orbit 最小の child_mask (octant0)"
        );
        let applied = TransformAwareSvdag::permute_node(&n2, 0, true, false);
        assert_eq!(
            applied.child_mask, canonical.child_mask,
            "tag を入力へ適用→ canonical の mask"
        );
        assert_eq!(
            applied.children, canonical.children,
            "tag を入力へ適用→ canonical の children 配列 (値ごと)"
        );
    }

    #[test]
    fn compose_tags_group_closure_axioms() {
        // D4 群の閉包性を代表元でピン (作用の忠実性は compose_tags doc 参照)。
        // mirror は involution: m∘m = 恒等 (列挙順最初の表現は (0,F,F))。
        for (mx, mz) in [(true, false), (false, true), (true, true)] {
            let m = TransformTag {
                rotate_y_90_count: 0,
                mirror_x: mx,
                mirror_z: mz,
            };
            let c = TransformAwareSvdag::compose_tags(m, m);
            assert_eq!(
                (c.rotate_y_90_count, c.mirror_x, c.mirror_z),
                (0, false, false),
                "({mx},{mz})∘({mx},{mz}) は恒等写像"
            );
        }
        // R90 の 4 乗は恒等: R90∘R90 = R180。16 記法では R180 同値の表現が
        // 2 つあり ((2,F,F) と (0,T,T) = XZ 全反転)、列挙順 (rot 外周) では
        // (0,T,T) が先に一致するためそちらが返る (決定的、検算で確認)。
        let r90 = TransformTag {
            rotate_y_90_count: 1,
            mirror_x: false,
            mirror_z: false,
        };
        let r180 = TransformAwareSvdag::compose_tags(r90, r90);
        assert_eq!(
            (r180.rotate_y_90_count, r180.mirror_x, r180.mirror_z),
            (0, true, true),
            "R90∘R90 = R180 ≡ (X,Z) 全反転 (列挙順最初の表現)"
        );
        let ident = TransformAwareSvdag::compose_tags(r180, r180);
        assert_eq!(
            (ident.rotate_y_90_count, ident.mirror_x, ident.mirror_z),
            (0, false, false),
            "R180∘R180 = 恒等"
        );
    }

    /// DW-5 (強化 pin, wave 120 系同型 4 件目): adversarial (b) — compose
    /// apply の作用順交換 (mirror 先行化) — が全既存 pin で検出不能だった
    /// 検出空白の補完。**非可換**ケース R90∘mirror_x で厳密値を固定:
    /// 手検算 (x,z)--R90-->(z,!x)--mirror_x-->(!z,!x)、列挙順最初の一致は
    /// rot=1,mx=T,mz=F (交換後解釈では (1,F,T) になり pin で RED)。
    /// さらに permute_node 契約整合 (2 段適用 == 合成 tag 1 段適用) も固定。
    #[test]
    fn compose_noncommutative_exact_tag_and_permute_consistency() {
        let r90 = TransformTag {
            rotate_y_90_count: 1,
            mirror_x: false,
            mirror_z: false,
        };
        let mx = TransformTag {
            rotate_y_90_count: 0,
            mirror_x: true,
            mirror_z: false,
        };
        let c = TransformAwareSvdag::compose_tags(r90, mx);
        assert_eq!(
            (c.rotate_y_90_count, c.mirror_x, c.mirror_z),
            (1, true, false),
            "R90∘mirror_x の列挙順最初表現は (1,T,F) (非可換で厳密)"
        );
        // permute 契約整合: 2 段適用 == 合成 tag の 1 段適用 (mask+children)
        for octant in [0usize, 5] {
            let n = node_at(octant, 9);
            let two_stage = TransformAwareSvdag::permute_node(
                &TransformAwareSvdag::permute_node(&n, 1, false, false),
                0,
                true,
                false,
            );
            let composed = TransformAwareSvdag::permute_node(&n, 1, true, false);
            assert_eq!(
                two_stage.child_mask, composed.child_mask,
                "oct{octant} mask"
            );
            assert_eq!(
                two_stage.children, composed.children,
                "oct{octant} children"
            );
            // 交換解釈 (mirror 先行、adversarial (b) の作用) とは**異なる**
            let swapped = {
                // mirror 先行: permute では rot も同時指定のため、別順序作用を
                // 明示構成して一致しないことを pin
                let step = TransformAwareSvdag::permute_node(&n, 0, true, false);
                TransformAwareSvdag::permute_node(&step, 1, false, false)
            };
            if octant == 5 {
                assert_ne!(
                    swapped.child_mask, composed.child_mask,
                    "mirror 先行 (変体 (b) の解釈) とは非可換で別作用"
                );
            }
        }
    }

    #[test]
    fn repeated_insert_is_deterministic_and_contract_preserving() {
        // 同一ノードの再挿入: 初回 (pool 登録) と 2 回目 (直接ヒット) で
        // (id, tag) が完全一致すること、および canonical ID が不変なことをピン。
        let mut tdag = TransformAwareSvdag::new();
        let n = node_at(6, 7); // y=1 octant: 変換は y 不変 → canonical は自身
        let first = tdag.insert_transform_aware(n.clone());
        let second = tdag.insert_transform_aware(n.clone());
        assert_eq!(first.0, second.0, "ID は再挿入で不変");
        assert_eq!(
            (
                first.1.rotate_y_90_count,
                first.1.mirror_x,
                first.1.mirror_z
            ),
            (
                second.1.rotate_y_90_count,
                second.1.mirror_x,
                second.1.mirror_z
            ),
            "tag も再挿入で完全一致 (memo ヒット経路)"
        );
        // octant6 = (x=F,y=T,z=T): y ビット不変の orbit は idx {6,7,3,2}、
        // mask 最小は octant2 (0b0000_0100)。child 値 7 が octant2 に移動すること、
        // および初回タグ = mirror_z ((0,F,T)、列挙順厳密再現で検算済) を固定。
        assert_eq!(
            (
                first.1.rotate_y_90_count,
                first.1.mirror_x,
                first.1.mirror_z
            ),
            (0, false, true),
            "octant6 の canonical 到達タグは mirror_z"
        );
        let canonical = &tdag.base_dag.nodes[first.0 as usize];
        assert_eq!(
            canonical.child_mask, 0b0000_0100,
            "canonical octant は idx2 = (x=F,y=T,z=F) で y 不変を保持"
        );
        assert_eq!(canonical.child_mask.count_ones(), 1, "単一 octant 維持");
        assert_eq!(
            canonical.children[2], 7,
            "child 値は octant 移動と共に運ばれる"
        );
    }

    // ================= wave 123 DW: 厳密契約ピン群 =================

    /// DW-3: canonical 一意性の経路非依存 pin — orbit {oct0,1,4,5} の
    /// 3 メンバー (5,1,4) どれから挿入しても同 ID・同 canonical 形。
    /// (insert_node dedup + orbit 閉包 + strict-min 列挙順決定性の連鎖)。
    #[test]
    fn canonical_is_path_independent_across_orbit_members() {
        let mut tdag = TransformAwareSvdag::new();
        let members = [node_at(5, 42), node_at(1, 42), node_at(4, 42)];
        let mut ids = Vec::new();
        for m in &members {
            ids.push(tdag.insert_transform_aware(m.clone()).0);
        }
        assert!(ids.iter().all(|&id| id == ids[0]), "orbit 3 経路で ID 一致");
        let canonical = &tdag.base_dag.nodes[ids[0] as usize];
        assert_eq!(canonical.child_mask, 1, "canonical は octant0 (mask 最小)");
        assert_eq!(canonical.children[0], 42, "child 値は octant0 に輸送");
        // 各メンバー挿入の返却 tag は契約を満たす (適用→canonical 一致)
        for (i, m) in members.iter().enumerate() {
            let td = TransformAwareSvdag::new();
            let _ = td; // 独立インスタンス比較は下で実施
            let tag = {
                let mut fresh = TransformAwareSvdag::new();
                fresh.insert_transform_aware(m.clone()).1
            };
            let applied = TransformAwareSvdag::permute_node(
                m,
                tag.rotate_y_90_count % 4,
                tag.mirror_x,
                tag.mirror_z,
            );
            assert_eq!(
                applied.child_mask, canonical.child_mask,
                "メンバー {i} の tag 適用→同 canonical mask"
            );
            assert_eq!(
                applied.children, canonical.children,
                "メンバー {i} の tag 適用→同 canonical children"
            );
        }
        // 独立インスタンス (n_c 単独挿入) でも canonical 形は一致
        let mut solo = TransformAwareSvdag::new();
        let solo_id = solo.insert_transform_aware(node_at(4, 42)).0;
        let solo_canonical = &solo.base_dag.nodes[solo_id as usize];
        assert_eq!(solo_canonical.child_mask, canonical.child_mask);
        assert_eq!(solo_canonical.children, canonical.children);
    }

    /// DW-4: y ビット不変性の直接 pin — y=1 octant (index 2,3,6,7) の
    /// ノードは全 16 変換で mask が y=1 領域に閉じ、occupancy も保存
    /// される (Y 面 D4 は y 軸を固定)。
    /// 領域 mask は rq dw_mask.rq で機械導出: y=1 = 0b1100_1100 (204)、
    /// y=0 = 0b0011_0011 (51)、cover=255・disjoint=0 の assert 通過。
    /// (捕捉 48: 初版は y=1 を 0b0100_0100 と誤記 → テスト赤が捕捉。
    /// oct2,6 しか含まない誤 mask で、実装は正しかった)
    #[test]
    fn y_bit_is_invariant_under_all_16_transforms() {
        const Y1: u8 = 0b1100_1100; // oct2,3,6,7 (rq 導出 204)
        const Y0: u8 = 0b0011_0011; // oct0,1,4,5 (rq 導出 51)
        assert_eq!(Y1 | Y0, 0xFF);
        assert_eq!(Y1 & Y0, 0);
        for octant in [2usize, 3, 6, 7] {
            let n = node_at(octant, 11);
            for rot in 0..4u8 {
                for mx in [false, true] {
                    for mz in [false, true] {
                        let p = TransformAwareSvdag::permute_node(&n, rot, mx, mz);
                        assert_ne!(
                            p.child_mask & Y1,
                            0,
                            "oct{octant} rot{rot} mx{mx} mz{mz}: y=1 領域を離れない"
                        );
                        assert_eq!(
                            p.child_mask & Y0,
                            0,
                            "oct{octant} rot{rot} mx{mx} mz{mz}: y=0 領域へは出ない"
                        );
                        assert_eq!(
                            p.child_mask.count_ones(),
                            1,
                            "oct{octant}: occupancy 保存 (単一 octant 維持)"
                        );
                    }
                }
            }
        }
        // 対側 (y=0 octant) も y=0 領域に閉じる (対称 pin)
        for octant in [0usize, 1, 4, 5] {
            let p = TransformAwareSvdag::permute_node(&node_at(octant, 3), 3, true, true);
            assert_eq!(p.child_mask & Y1, 0, "y=0 は y=1 領域へ出ない");
            assert_ne!(p.child_mask & Y0, 0, "y=0 領域内に留まる");
        }
    }
}
