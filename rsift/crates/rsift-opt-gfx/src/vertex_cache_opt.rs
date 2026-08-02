//! Vertex cache optimization — Forsyth's "Linear-Speed Vertex Cache Optimization".
//!
//! Reorders an indexed triangle list so recently-used vertices stay in the GPU's
//! small post-transform cache (typically 10–24 entries on integrated GPUs). This cuts
//! the average cache-miss ratio (ACMR) from ~3.0 toward ~1.2, i.e. far fewer vertex
//! shader invocations — a direct, hardware-agnostic win on weak GPUs.

#[derive(Clone, Copy, Debug)]
pub struct VertexCacheOptimizer {
    /// Simulated post-transform cache size (AMD/Intel iGPUs: ~16–32).
    pub cache_size: u32,
}

impl Default for VertexCacheOptimizer {
    fn default() -> Self {
        Self { cache_size: 24 }
    }
}

impl VertexCacheOptimizer {
    pub fn new(cache_size: u32) -> Self {
        Self {
            cache_size: cache_size.max(4),
        }
    }

    /// Per-vertex score from its simulated cache position (Forsyth scoring).
    #[inline]
    /// Tipsify (Forsyth/Sander) 式の頂点スコア。
    /// 「キャッシュ内の再利用」を strongest に評価し、未キャッシュは 0 とする。
    /// 未キャッシュを高得点にすると「新規頂点だらけの三角」を優先してしまい
    /// ACMR が悪化する (再利用だらけの三角より上になってはいけない)。
    fn vertex_score(&self, cache_pos: i32) -> f32 {
        if cache_pos < 0 {
            0.0 // キャッシュ不在 (valence boost は簡略化のため省略)
        } else if cache_pos < 3 {
            0.75 + 10.0 // 直近3頂点は強く引き付ける
        } else {
            let span = (self.cache_size as i32 - 3).max(1) as f32;
            let scaled = 1.0 - (cache_pos - 3) as f32 / span; // 古いほど 0 に近づく
            2.0 * scaled * scaled
        }
    }

    /// Reorder `indices` (flat triangle list) for optimal cache reuse.
    ///
    /// Contract: `indices.len()` must be a multiple of 3 (fail-loud; 以前は
    /// `chunks_exact(3)` が末尾の半端な 1-2 index を静寂に捨てていた)。
    /// `cache_size` は > 0 必須 (模擬キャッシュの末尾アクセスが underflow する)。
    pub fn optimize(&self, indices: &[u32]) -> Vec<u32> {
        assert!(
            indices.len() % 3 == 0,
            "optimize: indices.len() ({}) must be a multiple of 3 (flat triangle list; 端数 index を静寂 drop しない)",
            indices.len()
        );
        assert!(
            self.cache_size > 0,
            "optimize: cache_size must be > 0 (模擬キャッシュ配列が空)"
        );
        let ntri = indices.len() / 3;
        let mut tris: Vec<[u32; 3]> = Vec::with_capacity(ntri);
        for t in indices.chunks_exact(3) {
            tris.push([t[0], t[1], t[2]]);
        }
        let nv = indices
            .iter()
            .copied()
            .max()
            .map(|m| m as usize + 1)
            .unwrap_or(0);

        // Triangles touching each vertex (for incremental score updates).
        // CSR 形式 (offsets+flat): Vec<Vec> だと頂点数分の個別アロケーションが
        // 巨大メッシュで支配的になる。各頂点の三角列は ti 昇順 (面内では
        // v0,v1,v2 順) を維持するため、後続の更新順序は Vec<Vec> 版と同一。
        let mut vt_off = vec![0u32; nv + 1];
        for t in &tris {
            for &v in t {
                vt_off[v as usize + 1] += 1;
            }
        }
        for v in 0..nv {
            vt_off[v + 1] += vt_off[v];
        }
        let mut vt_flat = vec![0u32; vt_off[nv] as usize];
        {
            let mut cursor = vt_off[..nv].to_vec();
            for (ti, t) in tris.iter().enumerate() {
                for &v in t {
                    let c = &mut cursor[v as usize];
                    vt_flat[*c as usize] = ti as u32;
                    *c += 1;
                }
            }
        }

        let cs = self.cache_size as usize;
        let mut cache: Vec<i32> = vec![-1; cs]; // vertex id or -1, index = cache position
        let mut cache_pos = vec![-1i32; nv];
        let mut tri_emitted = vec![false; ntri];
        let mut tri_score = vec![0.0f32; ntri];

        // 頂点スコアは cache_pos ∈ {-1,0..cs} の cs+1 通りしかないため事前に
        // テーブル化する (値は vertex_score と bit 同一、呼び出し順も同じ)。
        let mut score_table = Vec::with_capacity(cs + 1);
        score_table.push(self.vertex_score(-1));
        for p in 0..cs as i32 {
            score_table.push(self.vertex_score(p));
        }
        let recompute = |ti: usize, cache_pos: &[i32]| -> f32 {
            let t = tris[ti];
            score_table[(cache_pos[t[0] as usize] + 1) as usize]
                + score_table[(cache_pos[t[1] as usize] + 1) as usize]
                + score_table[(cache_pos[t[2] as usize] + 1) as usize]
        };
        for ti in 0..ntri {
            tri_score[ti] = recompute(ti, &cache_pos);
        }

        // Use a vertex: move it to the front of the simulated cache.
        // wave 213 HJ: cache_pos の更新を「実際に位置が変わる区間」に限定
        // (従来は全 cs 要素を毎回無条件で再書き込みしていた)。書き換えた
        // セルと値は従来版と厳密に一致する (位置が変わらないセルは従来版も
        // 同じ値を上書きしていただけ)。
        let use_vertex = |v: u32, cache: &mut Vec<i32>, cache_pos: &mut Vec<i32>| {
            let mut found = -1i32;
            for k in 0..cache.len() {
                if cache[k] == v as i32 {
                    found = k as i32;
                    break;
                }
            }
            if found >= 0 {
                // 区間 [1..=found] が 1 つ後ろにずれる。それ以降は不変。
                // (区間内は定義上全て占有済みだが、空枠 -1 を cache_pos に
                // 書き込まない防御ガードは残す)
                for k in (1..=found as usize).rev() {
                    cache[k] = cache[k - 1];
                    if cache[k] >= 0 {
                        cache_pos[cache[k] as usize] = k as i32;
                    }
                }
            } else {
                // 追放される頂点の位置を無効化しないと「まだキャッシュ内」扱いされ
                // スコアが過大評価される (真の Tipsify と同様に LRU 追放を追跡)。
                let evicted = cache[cache.len() - 1];
                // 区間 [1..cs) が 1 つ後ろにずれ、末尾要素が追放される。
                // 末尾側は未占有 (-1) の場合があり、それを cache_pos には書かない。
                for k in (1..cache.len()).rev() {
                    cache[k] = cache[k - 1];
                    if cache[k] >= 0 {
                        cache_pos[cache[k] as usize] = k as i32;
                    }
                }
                if evicted >= 0 {
                    cache_pos[evicted as usize] = -1;
                }
            }
            cache[0] = v as i32;
            cache_pos[v as usize] = 0;
        };

        // 候補キュー (世代番号つき遅延無効化の優先度キュー)。
        // 素朴な全走査は O(ntri^2) になり大きなメッシュで破綻する。
        // 三角の version は放出に伴う再スコア時にだけ進め、そのたびに
        // 「現在 version のエントリ」を新たに積む。よって未放出の各三角には
        // 有効エントリがちょうど1つ存在し、古いエントリは pop 時に捨てるだけで
        // 終了が保証される (積み直すと最新エントリが即 stale 化して飢餓する)。
        // 更新回数は Σ(valence) に比例し、準線形に抑えられる。
        //
        // wave 213 HJ: 同一意味論の定数倍最適化 (出力 bit 一致の証明付き高速化)。
        // (a) ソートキーを単一 u64 に畳み込み — score は常に非負の有限 f32
        //     (vertex_score は 0.0/10.75/2s² のみ) なので to_bits() が全順序を
        //     厳密保存し、同点規則「tri 降順」は下位 32bit の tri が担う。
        //     ヒープ比較は total_cmp×2 から u64 1 命令になる。
        // (c) 再計算で値が変わらない場合は version 不変・push 省略 —
        //     従来版も同値の末尾エントリを積むだけで有効エントリの値集合は
        //     不変なので、積まなくても全く同じ pop 系列になる。
        //     (stale エントリの個数差は捨て工作業量だけに影響し、ver 検査で
        //     必ず弾かれるため出力に不干渉)
        //
        // 設計上の正直注記 (HJ-1): 「emit 三角の 3 頂点を先に一括移動して
        // 隣接再計算を 1 度化する」案は**採用しない**。LRU 追放が非共有
        // 頂点にも及び、従来版は「既に再計算した三角には追放の影響を伝播
        // しない」遅延更新のスタレネスを仕様として含む (BV-3 ピンが厳密系列で
        // この挙動を固定)。一括化はこのスタレネスを消すため bit 一致にならず
        // (optimality_reduces_acmr で検出済)、頂点ごとの逐次処理順序は維持する。
        #[derive(Clone, Copy)]
        struct Cand {
            key: u64,
            tri: u32,
            ver: u32,
        }
        impl PartialEq for Cand {
            fn eq(&self, o: &Self) -> bool {
                self.key == o.key && self.tri == o.tri
            }
        }
        impl Eq for Cand {}
        impl PartialOrd for Cand {
            fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(o))
            }
        }
        impl Ord for Cand {
            fn cmp(&self, o: &Self) -> std::cmp::Ordering {
                // key 下位 32bit が tri なので key だけで全順序は完結するが、
                // 等価比較の意味を明示するため tri も順序に含める (key が
                // 異なるケースでは key 比較が先に決着するため結果は不変)。
                self.key.cmp(&o.key).then_with(|| self.tri.cmp(&o.tri))
            }
        }
        let cand_key =
            |score: f32, tri: u32| -> u64 { ((score.to_bits() as u64) << 32) | tri as u64 };
        let mut version = vec![0u32; ntri];
        // collect で bottom-up heapify (O(n)) にする。ヒープ上の優先度は
        // (score, tri) の全順序で一意 (tri は相異なる) ため、pop 列は
        // push 版と bit 同一 (ヒープ内部配列の形は結果に影響しない)。
        let mut heap: std::collections::BinaryHeap<Cand> = (0..ntri)
            .map(|ti| Cand {
                key: cand_key(tri_score[ti], ti as u32),
                tri: ti as u32,
                ver: 0,
            })
            .collect();
        let mut out: Vec<u32> = Vec::with_capacity(indices.len());
        let mut emitted = 0usize;
        while emitted < ntri {
            let Cand { tri, ver, .. } = heap.pop().expect("heap must not drain before all emitted");
            let ti = tri as usize;
            if tri_emitted[ti] {
                continue;
            }
            if ver != version[ti] {
                // 古いエントリは捨てるだけ (再スコアして積み直すと、その積み直しが
                // version を進めて直近の有効エントリまで stale 化し、同点スコアの
                // ヒープで有効エントリが永久に pop されない無限ループになり得る)。
                // 有効エントリは version 更新時に必ず積まれているので消滅しない。
                continue;
            }
            tri_emitted[ti] = true;
            emitted += 1;
            let t = tris[ti];
            for &v in &t {
                use_vertex(v, &mut cache, &mut cache_pos);
                // Recompute scores of triangles sharing v (CSR 区間走査)。
                // wave 213 HJ (c): 値が変わらない再スコアは version/push を
                // 省略する (従来は touch のたびに無条件で積んでいた)。
                for &ot in &vt_flat[vt_off[v as usize] as usize..vt_off[v as usize + 1] as usize] {
                    let oti = ot as usize;
                    if tri_emitted[oti] {
                        continue;
                    }
                    let s = recompute(oti, &cache_pos);
                    if s != tri_score[oti] {
                        version[oti] = version[oti].wrapping_add(1);
                        tri_score[oti] = s;
                        heap.push(Cand {
                            key: cand_key(s, ot),
                            tri: ot,
                            ver: version[oti],
                        });
                    }
                }
            }
            out.extend_from_slice(&t);
        }
        out
    }

    /// Average Cache Miss Ratio = transformed vertices / triangles。
    /// `optimize` と同じ LRU モデルでシミュレートする (ヒット時に MRU へ昇格、
    /// 追放時に位置を無効化)。以前は追放頂点の `pos` が残りミスを過少計上して
    /// いた。
    ///
    /// Contract: `indices.len()` は 3 の倍数 (fail-loud、`optimize` と同一)。
    /// `cache_size` は > 0 必須 — 0 だと `cache[cs - 1]` が usize underflow
    /// (debug: subtract overflow panic / release: OOB panic) し、エラー原因が
    /// 不明瞭になるため入口で明示する (`new` は .max(4) で安全だが `acmr` は
    /// 生の u32 を直接受け取る)。
    pub fn acmr(indices: &[u32], cache_size: u32) -> f32 {
        assert!(cache_size > 0, "acmr: cache_size must be > 0");
        assert!(
            indices.len() % 3 == 0,
            "acmr: indices.len() ({}) must be a multiple of 3",
            indices.len()
        );
        let ntri = indices.len() / 3;
        if ntri == 0 {
            return 0.0;
        }
        let cs = cache_size as usize;
        let mut cache: Vec<i32> = vec![-1; cs];
        let mut pos = vec![-1i32; (indices.iter().copied().max().unwrap_or(0) as usize) + 1];
        let mut misses = 0u32;
        for w in indices {
            let v = *w as usize;
            if pos[v] >= 0 {
                // LRU ヒット: MRU (先頭) へ移動
                let cur = pos[v] as usize;
                for k in (0..cur).rev() {
                    cache[k + 1] = cache[k];
                }
                cache[0] = v as i32;
            } else {
                misses += 1;
                // LRU 追放: 末尾の頂点を無効化
                let evicted = cache[cs - 1];
                if evicted >= 0 {
                    pos[evicted as usize] = -1;
                }
                for k in (0..cs.saturating_sub(1)).rev() {
                    cache[k + 1] = cache[k];
                }
                cache[0] = v as i32;
            }
            for k in 0..cs {
                if cache[k] >= 0 {
                    pos[cache[k] as usize] = k as i32;
                }
            }
        }
        misses as f32 / ntri as f32
    }

    pub fn wgsl_source(&self) -> &'static str {
        VERTEX_CACHE_OPT_WGSL
    }
}

pub const VERTEX_CACHE_OPT_WGSL: &str = include_str!("../shaders/vertex_cache_opt.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    // A 2x2 grid of quads expressed as triangles with vertex reuse.
    fn grid_indices() -> Vec<u32> {
        // vertices 0..8 laid out 3x3, two tris per cell, 4 cells.
        let cell = |x: u32, y: u32| -> [u32; 4] {
            let b = y * 3 + x;
            [b, b + 1, b + 3, b + 4]
        };
        let mut idx = Vec::new();
        for y in 0..2u32 {
            for x in 0..2u32 {
                let [a, b, c, d] = cell(x, y);
                idx.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }
        idx
    }

    // セル数 `cells`x`cells` のグリッド (頂点・三角を共有)。
    fn grid_indices_n(cells: u32) -> Vec<u32> {
        let mut idx = Vec::new();
        for y in 0..cells {
            for x in 0..cells {
                let b = y * (cells + 1) + x;
                let (c, d) = (b + (cells + 1), b + (cells + 2));
                idx.extend_from_slice(&[b, c, b + 1, b + 1, c, d]);
            }
        }
        idx
    }

    #[test]
    fn optimality_reduces_acmr() {
        // 4x4 セル (25頂点, 32三角) を cache=8 で評価。
        // 注意: 頂点数 <= cache サイズだと全頂点が1回のミスのみで ACMR は
        // 理論下限に達し「改善不可能」になるため、cache < 頂点数で測る。
        let idx = grid_indices_n(4);
        let opt = VertexCacheOptimizer::new(8);
        let before = VertexCacheOptimizer::acmr(&idx, 8);
        let reordered = opt.optimize(&idx);
        let after = VertexCacheOptimizer::acmr(&reordered, 8);
        assert!(after < before, "ACMR {after} should beat {before}");
        assert!(
            after < 2.0,
            "optimized ACMR should be well under 3.0, got {after}"
        );
    }

    #[test]
    fn reordered_has_same_triangles() {
        let idx = grid_indices();
        let opt = VertexCacheOptimizer::new(16);
        let r = opt.optimize(&idx);
        assert_eq!(r.len(), idx.len());
        // Same multiset of (unordered) triangles.
        let mut a: Vec<[u32; 3]> = idx
            .chunks_exact(3)
            .map(|c| {
                let mut x = [c[0], c[1], c[2]];
                x.sort();
                x
            })
            .collect();
        let mut b: Vec<[u32; 3]> = r
            .chunks_exact(3)
            .map(|c| {
                let mut x = [c[0], c[1], c[2]];
                x.sort();
                x
            })
            .collect();
        a.sort();
        b.sort();
        assert_eq!(a, b);
    }

    #[test]
    fn worst_case_unoptimized_is_high() {
        // No reuse → ACMR ≈ 3.0.
        let idx: Vec<u32> = (0..30u32).collect(); // 10 disjoint triangles
        let acmr = VertexCacheOptimizer::acmr(&idx, 16);
        assert!((acmr - 3.0).abs() < 1e-3, "got {acmr}");
    }

    #[test]
    fn emits_exact_sequence_empty_cache_tie() {
        // BV-3a 手導出ピン: 全スコア 0 同点 → heap 規則「スコア降順、同点は
        // tri 番号降順」(BinaryHeap max-heap, `Ord` doc) により tri 1 を先に
        // pop する。放出後も頂点共有が無いので再スコアは起きず tri 0 が続く。
        let opt = VertexCacheOptimizer::new(4);
        let out = opt.optimize(&[0, 1, 2, 3, 4, 5]);
        assert_eq!(out, vec![3, 4, 5, 0, 1, 2]);
    }

    #[test]
    fn emits_exact_sequence_shared_edge_rescore() {
        // BV-3b 手導出ピン: 共有辺 (v1, v2)。初回 pop は同点規則で T1。
        // use_vertex(2),(1),(3) 後 cache=[3,1,2,-1]、T0 の cache_pos は
        // v0=-1(→0.0), v1=1, v2=2 (直近3枠 → 各 0.75+10=10.75) で再スコア
        // 0+10.75+10.75=21.5 となり T0 が続けて放出される。世代番号つき heap の
        // stale エントリは skip され、更新なし再スキャンでも飢餓しない。
        let opt = VertexCacheOptimizer::new(4);
        let out = opt.optimize(&[0, 1, 2, 2, 1, 3]);
        assert_eq!(out, vec![2, 1, 3, 0, 1, 2]);
    }

    #[test]
    fn acmr_exact_pins_lru_model() {
        // 独立導出ピン: LRU 手順 (ヒット時 MRU 昇格・末尾追放の位置無効化) を
        // 手計算で追跡した厳密値。misses/ntri は整数比 → f32 誤差ゼロ。
        // [3,4,5,0,1,2]: 全ミス 6/2 = 3.0 (ワースト)
        assert_eq!(VertexCacheOptimizer::acmr(&[3, 4, 5, 0, 1, 2], 4), 3.0);
        // [2,1,3,0,1,2]: v2,v1,v3,v0 ミス → v1,v2 ヒット = 4/2 = 2.0
        assert_eq!(VertexCacheOptimizer::acmr(&[2, 1, 3, 0, 1, 2], 4), 2.0);
    }

    #[test]
    #[should_panic(expected = "cache_size must be > 0")]
    fn acmr_rejects_zero_cache_size() {
        // BV-2: cache[cs-1] の usize underflow (debug: subtract overflow /
        // release: OOB) による不明瞭パニックを入口契約で明示する。
        let _ = VertexCacheOptimizer::acmr(&[0, 1, 2], 0);
    }

    #[test]
    #[should_panic(expected = "multiple of 3")]
    fn optimize_rejects_partial_triangle() {
        // BV-1: chunks_exact(3) が末尾 1-2 index を静寂 drop する経路を
        // fail-loud 契約に転換 (契約違反は誤用であり即座に検出すべき)。
        let _ = VertexCacheOptimizer::new(4).optimize(&[0, 1, 2, 3]);
    }

    #[test]
    #[should_panic(expected = "multiple of 3")]
    fn acmr_rejects_partial_triangle() {
        let _ = VertexCacheOptimizer::acmr(&[0, 1, 2, 3], 8);
    }

    #[test]
    fn wgsl_note_mirrors_vertex_score_vocabulary() {
        // BV-4: vertex_cache_opt.wgsl の CacheScore は vertex_score の逐語
        // ミラー (dispatch なしの注記ファイルだが、例示式が Rust 実装と値不一致
        // だと誤誘導するため契約語彙でピンする)。
        let wgsl = VERTEX_CACHE_OPT_WGSL;
        assert!(wgsl.contains("0.75 + 10.0"), "直近3枠スコア (10.75) の語彙");
        assert!(wgsl.contains("2.0 * scaled * scaled"), "減衰 2次式の語彙");
        assert!(wgsl.contains("max(cacheSize - 3, 1)"), "span clamp の語彙");
        assert!(wgsl.contains("return 0.0;"), "未キャッシュ=0 の語彙");
    }
}
