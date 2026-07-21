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
        Self { cache_size: cache_size.max(4) }
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
    pub fn optimize(&self, indices: &[u32]) -> Vec<u32> {
        let ntri = indices.len() / 3;
        let mut tris: Vec<[u32; 3]> = Vec::with_capacity(ntri);
        for t in indices.chunks_exact(3) {
            tris.push([t[0], t[1], t[2]]);
        }
        let nv = indices.iter().copied().max().map(|m| m as usize + 1).unwrap_or(0);

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
        let use_vertex = |v: u32, cache: &mut Vec<i32>, cache_pos: &mut Vec<i32>| {
            let mut found = -1i32;
            for k in 0..cache.len() {
                if cache[k] == v as i32 {
                    found = k as i32;
                    break;
                }
            }
            if found >= 0 {
                for k in (0..found as usize).rev() {
                    cache[k + 1] = cache[k];
                }
            } else {
                // 追放される頂点の位置を無効化しないと「まだキャッシュ内」扱いされ
                // スコアが過大評価される (真の Tipsify と同様に LRU 追放を追跡)。
                let evicted = cache[cache.len() - 1];
                if evicted >= 0 {
                    cache_pos[evicted as usize] = -1;
                }
                for k in (0..cache.len().saturating_sub(1)).rev() {
                    cache[k + 1] = cache[k];
                }
            }
            cache[0] = v as i32;
            for k in 0..cache.len() {
                if cache[k] >= 0 {
                    cache_pos[cache[k] as usize] = k as i32;
                }
            }
        };

        // 候補キュー (世代番号つき遅延無効化の優先度キュー)。
        // 素朴な全走査は O(ntri^2) になり大きなメッシュで破綻する。
        // 三角の version は放出に伴う再スコア時にだけ進め、そのたびに
        // 「現在 version のエントリ」を新たに積む。よって未放出の各三角には
        // 有効エントリがちょうど1つ存在し、古いエントリは pop 時に捨てるだけで
        // 終了が保証される (積み直すと最新エントリが即 stale 化して飢餓する)。
        // 更新回数は Σ(valence) に比例し、準線形に抑えられる。
        #[derive(Clone, Copy)]
        struct Cand {
            score: f32,
            tri: u32,
            ver: u32,
        }
        impl PartialEq for Cand {
            fn eq(&self, o: &Self) -> bool {
                self.score.total_cmp(&o.score) == std::cmp::Ordering::Equal && self.tri == o.tri
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
                // 決定的: スコア降順 → 同点は tri 番号降順
                self.score.total_cmp(&o.score).then_with(|| self.tri.cmp(&o.tri))
            }
        }
        let mut version = vec![0u32; ntri];
        // collect で bottom-up heapify (O(n)) にする。ヒープ上の優先度は
        // (score, tri) の全順序で一意 (tri は相異なる) ため、pop 列は
        // push 版と bit 同一 (ヒープ内部配列の形は結果に影響しない)。
        let mut heap: std::collections::BinaryHeap<Cand> = (0..ntri)
            .map(|ti| Cand { score: tri_score[ti], tri: ti as u32, ver: 0 })
            .collect();

        let mut out: Vec<u32> = Vec::with_capacity(indices.len());
        let mut emitted = 0usize;
        while emitted < ntri {
            let Cand { score: _, tri, ver } =
                heap.pop().expect("heap must not drain before all emitted");
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
                for &ot in &vt_flat[vt_off[v as usize] as usize..vt_off[v as usize + 1] as usize] {
                    let oti = ot as usize;
                    if !tri_emitted[oti] {
                        version[oti] = version[oti].wrapping_add(1);
                        let s = recompute(oti, &cache_pos);
                        tri_score[oti] = s;
                        heap.push(Cand { score: s, tri: ot, ver: version[oti] });
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
    pub fn acmr(indices: &[u32], cache_size: u32) -> f32 {
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
        assert!(after < 2.0, "optimized ACMR should be well under 3.0, got {after}");
    }

    #[test]
    fn reordered_has_same_triangles() {
        let idx = grid_indices();
        let opt = VertexCacheOptimizer::new(16);
        let r = opt.optimize(&idx);
        assert_eq!(r.len(), idx.len());
        // Same multiset of (unordered) triangles.
        let mut a: Vec<[u32; 3]> = idx.chunks_exact(3).map(|c| { let mut x=[c[0],c[1],c[2]]; x.sort(); x }).collect();
        let mut b: Vec<[u32; 3]> = r.chunks_exact(3).map(|c| { let mut x=[c[0],c[1],c[2]]; x.sort(); x }).collect();
        a.sort(); b.sort();
        assert_eq!(a, b);
    }

    #[test]
    fn worst_case_unoptimized_is_high() {
        // No reuse → ACMR ≈ 3.0.
        let idx: Vec<u32> = (0..30u32).collect(); // 10 disjoint triangles
        let acmr = VertexCacheOptimizer::acmr(&idx, 16);
        assert!((acmr - 3.0).abs() < 1e-3, "got {acmr}");
    }
}
