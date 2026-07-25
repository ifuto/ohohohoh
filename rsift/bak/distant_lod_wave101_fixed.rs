//! Distant Horizons 完成形 — 量子ツリー高度カラム + スカート付き LOD メッシュ生成。
//!
//! DH の仕組み:
//! - 高解像ワールド → カラム単位の top height + color の heightmap
//! - 2x2 を 1 にするダウンサンプルで LOD 階層 (Ocean)
//! - 距離に応じて LOD を選び、段差の隙間を「スカート」で塞ぐ
//! - 色は 4 近傍から最多頻度 (mode) で補間ずれを抑止
//!
//! 出力: レンダに又一パスで流用可能な GPU 頂点レイアウト
//! (pos pack 16bit, color RGBA8) + index (u32)。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnSample {
    /// 最上 opaque Y (0 = 空)
    pub top_y: u16,
    /// その色 (RGBA8 pack)
    pub color: u32,
    /// 水が覆っているか (透過面を出すため)
    pub underwater: bool,
}

impl ColumnSample {
    pub const fn air() -> Self {
        Self { top_y: 0, color: 0, underwater: false }
    }
}

/// 1 リージョン (N x N カラム)。`downsample` 時は各段で幅/高さが偶数
/// (= 全段で 2 の累乗に限る運用が素直) である必要がある。
/// `samples.len() == width * height` は全 API の前提不変量。
pub struct ColumnHeightmap {
    pub samples: Vec<ColumnSample>,
    pub width: u32,
    pub height: u32,
    /// ワールド原点 (ブロック単位)
    pub origin_x: i32,
    pub origin_z: i32,
}

impl ColumnHeightmap {
    pub fn get(&self, x: u32, z: u32) -> ColumnSample {
        let x = x.min(self.width - 1);
        let z = z.min(self.height - 1);
        self.samples[(z * self.width + x) as usize]
    }
}

/// LOD 1 段 = 2x2 → 1。出力は次段高さマップ。
///
/// **契約 (wave 101 DA-1)**:
/// - `samples.len() == width * height` は構造不変量 (旧実装は `get` の範囲外
///   アクセスか、長い Vec では静的な誤領域参照かのどちらかを黙って起こした)。
/// - `width` / `height` は 1 以下か偶数必須。3 以上の奇数は末尾の列・行が
///   2x2 走査の範囲外 (= 縁のカラムが出力次段へ**静寂に脱落**) するため拒否
///   する (モジュール doc の「N は 2 の累乗でなくてもよい」はこの脱落を
///   含意しない範囲でのみ成立する主張に訂正)。
pub fn downsample(src: &ColumnHeightmap) -> ColumnHeightmap {
    assert!(
        src.samples.len() as u64 == src.width as u64 * src.height as u64,
        "downsample: samples.len() ({}) != width*height ({}x{}) (構造不変量違反)",
        src.samples.len(),
        src.width,
        src.height
    );
    assert!(
        (src.width <= 1 || src.width % 2 == 0) && (src.height <= 1 || src.height % 2 == 0),
        "downsample: 幅/高さは 1 以下か偶数必須 ({}x{}): 奇数だと縁のカラムが静寂に脱落する",
        src.width,
        src.height
    );
    let w2 = src.width / 2;
    let h2 = src.height / 2;
    if w2 == 0 || h2 == 0 {
        return ColumnHeightmap {
            samples: vec![],
            width: 0,
            height: 0,
            origin_x: src.origin_x,
            origin_z: src.origin_z,
        };
    }
    let mut samples = Vec::with_capacity((w2 * h2) as usize);
    for z in 0..h2 {
        for x in 0..w2 {
            let a = src.get(2 * x, 2 * z);
            let b = src.get(2 * x + 1, 2 * z);
            let c = src.get(2 * x, 2 * z + 1);
            let d = src.get(2 * x + 1, 2 * z + 1);
            samples.push(merge_4(a, b, c, d));
        }
    }
    ColumnHeightmap {
        samples,
        width: w2,
        height: h2,
        origin_x: src.origin_x,
        origin_z: src.origin_z,
    }
}

/// DH の「4 近傍最多色 + 最頻高さ」。
fn merge_4(a: ColumnSample, b: ColumnSample, c: ColumnSample, d: ColumnSample) -> ColumnSample {
    let present = [a, b, c, d]
        .iter()
        .filter(|s| s.top_y > 0)
        .count();
    if present == 0 {
        return ColumnSample::air();
    }
    // 代表高さ = 最大値 (DH は粗い LOD 側で max を使う。absent (top_y == 0)
    // は 1 つでも存在があれば max は正になるのでここでは特別扱い不要)。
    let top_y = [a.top_y, b.top_y, c.top_y, d.top_y].into_iter().max().unwrap();
    let underwater = [a.underwater, b.underwater, c.underwater, d.underwater]
        .iter()
        .any(|u| *u);
    // 最多頻度色
    let mut color_counts: [(u32, u32); 4] = [(0, 0); 4];
    let mut n = 0;
    for s in [a, b, c, d] {
        if s.top_y == 0 {
            continue;
        }
        let mut found = false;
        for c0 in color_counts[..n].iter_mut() {
            if c0.0 == s.color {
                c0.1 += 1;
                found = true;
            }
        }
        if !found && n < 4 {
            color_counts[n] = (s.color, 1);
            n += 1;
        }
    }
    // 最多頻度色。**同票タイは走査順で後の色が勝つ** (max_by_key は「同値の
    // 最大が複数あるとき最後の要素を返す」公式仕様に依存する確定的挙動。
    // テストコメントが「先着」と誤記していた時代があったが実際は後勝ち)。
    let best = color_counts[..n]
        .iter()
        .max_by_key(|(_, k)| *k)
        .map(|(c, _)| *c)
        .unwrap_or(0);

    ColumnSample { top_y, color: best, underwater }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DistantVertex {
    /// block review からの 16bit pack (位置)
    pub pos_packed: [u16; 3],
    /// 予約 (face / AO / water フラグ用) — Pod はパディング禁止のため詰める
    pub flags: u16,
    /// RGBA8 パック色
    pub color: u32,
}

pub struct LodMesh {
    pub vertices: Vec<DistantVertex>,
    pub indices: Vec<u32>,
    pub lod_level: u32,
    /// ブロック単位での 1 quad の幅 (2^lod)
    pub cell_size: u32,
}

impl LodMesh {
    /// LOD マップから skirt 付きメッシュを生成。
    ///
    /// セル (x,z):
    ///   top quad: 4頂点 (セル角部の Y は 4 近傍の平均で補間)
    ///   skirt: 東西南北に「鉛直接地スカート」を付けて LOD 段差の隙間を
    ///   DH と同様に塞ぐ。深度は `min(4, max(min_y, 1))` ∈ [1, 4] m
    ///   (旧 doc の「4-8m 相当」は実式と不一致だった)
    ///
    /// **出力座標はマップローカル (ブロック単位, 原点 = map の南西角)**:
    /// `pos_packed` は [x*cell, y, z*cell] の u16 triple で、world への再配置は
    /// GPU 側が `origin_x/z` uniform で行う前提 (wave 101 DA-3。旧実装は
    /// `origin` を差し引く設計だったがローカル座標から引いていたため
    /// `origin != 0` のマップでは全頂点が負側へ飽和し 0 平面へ崩壊していた。
    /// 併せて整数ドメイン化で |座標| > 2^24 の f32 精度欠落経路を根絶)。
    ///
    /// **契約 (fail-loud)**:
    /// - `lod_level < 16` (cell = 2^lod が u16 span に収まる上限)
    /// - 構造不変量 `samples.len() == width * height`
    /// - `width * cell <= 65535` かつ `height * cell <= 65535` (pos_packed の
    ///   u16 span。旧実装は範囲外を境界へ**静寂クランプ**していた)
    /// - セル数 ≤ 214,748,364 (20 verts/セルが u32 index 空間に収まる上限。
    ///   これは構造不変量保証下の samples 長でも現実には拘束しないが、
    ///   到達すると `indexes` の base が静寂に折り返すため明示拒否する)
    /// - 0 次元マップは空メッシュを返す
    pub fn build(map: &ColumnHeightmap, lod_level: u32, min_y: f32) -> Self {
        assert!(
            lod_level < 16,
            "build: lod_level ({lod_level}) >= 16 は cell が u16 span を超過"
        );
        let cell = 1u32 << lod_level;
        assert!(
            map.samples.len() as u64 == map.width as u64 * map.height as u64,
            "build: samples.len() ({}) != width*height ({}x{}) (構造不変量違反)",
            map.samples.len(),
            map.width,
            map.height
        );
        assert!(
            map.width as u64 * cell as u64 <= 65535 && map.height as u64 * cell as u64 <= 65535,
            "build: 範囲 {}x{} cell={} が pos_packed u16 span (65535) を超過 (旧来は静寂クランプ)",
            map.width,
            map.height,
            cell
        );
        assert!(
            map.width as u64 * map.height as u64 <= 214_748_364,
            "build: セル数 {} が u32 index 空間上限 (20 verts/セル) を超過",
            map.width as u64 * map.height as u64
        );
        let mut vertices: Vec<DistantVertex> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let skirt_depth = 4.0f32.min(min_y.max(1.0));

        let push_quad = |verts: &mut Vec<DistantVertex>, indices: &mut Vec<u32>, quad: [DistantVertex; 4]| {
            let base = verts.len() as u32;
            verts.extend_from_slice(&quad);
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        };

        for z in 0..map.height as usize {
            for x in 0..map.width as usize {
                let c = map.get(x as u32, z as u32);
                if c.top_y == 0 {
                    continue;
                }
                // ローカル座標は整数ドメイン (ブロック単位, 2^24 精度壁の根絶)
                let bx = x as u32 * cell;
                let bz = z as u32 * cell;

                // 上面: 4 角を近傍平均で (境界で隣のLODに滑らかに合わせる)
                let y00 = neighbour_avg_y(map, x, z);
                let y10 = neighbour_avg_y(map, x + 1, z);
                let y01 = neighbour_avg_y(map, x, z + 1);
                let y11 = neighbour_avg_y(map, x + 1, z + 1);
                // wave 101 DA-2: 頂点の配置順は (x,z) → (x,z+1) → (x+1,z+1) →
                // (x+1,z) であり、対応する角 Y は [y00, y01, y11, y10]。
                // 旧実装は [y00, y10, y11, y01] と対角 2 角の高さを取り違えて
                // おり、傾斜のある全セルで上面がねじれたサドルになっていた
                // (flat 地形テストでは検出不能。Python 厳密シムで対応を確定)。
                let q = [
                    mkv(y00, c.color, bx, bz),
                    mkv(y01, c.color, bx, bz + cell),
                    mkv(y11, c.color, bx + cell, bz + cell),
                    mkv(y10, c.color, bx + cell, bz),
                ];
                // 上面の示す法線が上なら採用
                push_quad(&mut vertices, &mut indices, q);

                // まわりスカート（4 辺）
                let edges: [([u32; 2], [u32; 2]); 4] = [
                    ([bx, bz], [bx + cell, bz]),
                    ([bx + cell, bz], [bx + cell, bz + cell]),
                    ([bx + cell, bz + cell], [bx, bz + cell]),
                    ([bx, bz + cell], [bx, bz]),
                ];
                let edge_ys = [(y00, y10), (y10, y11), (y11, y01), (y01, y00)];
                for ((p0, p1), (ya, yb)) in edges.into_iter().zip(edge_ys.into_iter()) {
                    let sa = (ya - skirt_depth).max(0.0);
                    let sb = (yb - skirt_depth).max(0.0);
                    // 上2頂点は上面に一致、下2頂点は深めに落ちる; 法線は外向
                    let q2 = [
                        mkv(ya, c.color, p0[0], p0[1]),
                        mkv(yb, c.color, p1[0], p1[1]),
                        mkv(sb, c.color, p1[0], p1[1]),
                        mkv(sa, c.color, p0[0], p0[1]),
                    ];
                    push_quad(&mut vertices, &mut indices, q2);
                }
            }
        }

        Self { vertices, indices, lod_level, cell_size: cell }
    }

    pub fn triangle_count(&self) -> u32 {
        (self.indices.len() / 3) as u32
    }
}

/// 出力量: `pos_packed` はマップローカル (ブロック単位, 原点 = map 南西角) の
/// u16 pack。`lx`/`lz` は build 側の契約で [0, 65535] 保証済み。
///
/// Y の量子化は最近接丸め (wave 101 DA-5): 旧実装の `y as i32` は負方向への
/// 切り捨てで、近傍平均が生む .5 刻みの補間値を平均 0.5m 分傾斜側へ常に
/// 下げるバイアスがあった (LOD 段差の隙間 = まさにスカートで塞ぐべき相)。
/// `f32::round` は半は 0 から遠い側 (15.5 → 16) で偏りを最小化する。
#[inline]
fn mkv(y: f32, color: u32, lx: u32, lz: u32) -> DistantVertex {
    debug_assert!(
        lx <= 65535 && lz <= 65535,
        "mkv: ローカル座標が u16 span 外"
    );
    DistantVertex {
        pos_packed: [
            lx as u16,
            (y.round() as i32).clamp(0, 65535) as u16,
            lz as u16,
        ],
        flags: 0,
        color,
    }
}

/// DH 式周辺補間: 呼び出し側が受け渡す `map` は当該 LOD 段にダウンサンプル
/// 済みのマップ (= ±1 セルが 2^lod スケールの近傍に対応) であることが契約
/// (旧シグネチャの `_lod` 引数は未使用の死引数だったため除去)。
/// マップ縁では窓がクランプされ同じカラムが複数カウントされる (= 縁側への
/// 確定的な重み付け) 点に注意。`n == 0` 分岐は呼出文脈 (自セルが必ず窓内)
/// では到達不能の防御コード。
fn neighbour_avg_y(map: &ColumnHeightmap, x: usize, z: usize) -> f32 {
    let x0 = x.saturating_sub(1) as u32;
    let z0 = z.saturating_sub(1) as u32;
    let x1 = (x as u32).min(map.width - 1);
    let z1 = (z as u32).min(map.height - 1);
    let mut sum = 0f32;
    let mut n = 0f32;
    for zz in [z0, z1] {
        for xx in [x0, x1] {
            let s = map.get(xx, zz);
            if s.top_y > 0 {
                sum += s.top_y as f32;
                n += 1.0;
            }
        }
    }
    if n > 0.0 {
        sum / n
    } else {
        0.0
    }
}

/// 距離→LOD レベル選択 (DH の既定 table 相当)。
/// 境界は全て「以上」で次段側: 128 -> 1, 256 -> 2, 512 -> 3, 1024 -> 4, 2048 -> 5。
///
/// **契約 (wave 101 DA-4)**: `dist_blocks` は有限かつ非負必須。
/// 旧実装は NaN が全 `<` 比較を false にして**最遠 LOD 5 へ静寂に逃げる**
/// (= 近景が最低詳細化) 経路があった。wave 71 BU-1 (lod_hybrid) と同哲学で
/// 入口 fail-loud に遮断する (現消費者の chunk_dists は i32 座標の hypot
/// 由来で必ず有限・非負、配線済み経路には非発火を照合済)。
pub fn lod_for_distance(dist_blocks: f32) -> u32 {
    assert!(
        dist_blocks.is_finite() && dist_blocks >= 0.0,
        "lod_for_distance 契約違反: dist_blocks={dist_blocks} (NaN の静寂な最遠 LOD 化を遮断)"
    );
    if dist_blocks < 128.0 {
        0
    } else if dist_blocks < 256.0 {
        1
    } else if dist_blocks < 512.0 {
        2
    } else if dist_blocks < 1024.0 {
        3
    } else if dist_blocks < 2048.0 {
        4
    } else {
        5
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: u32, h: u32, y: u16) -> ColumnHeightmap {
        ColumnHeightmap {
            samples: vec![ColumnSample { top_y: y, color: 0xFF00FF00, underwater: false }; (w * h) as usize],
            width: w,
            height: h,
            origin_x: 0,
            origin_z: 0,
        }
    }

    #[test]
    fn downsample_keeps_max_height() {
        let mut m = flat(2, 2, 64);
        m.samples[0].top_y = 100;
        let d = downsample(&m);
        assert_eq!(d.width, 1);
        assert_eq!(d.get(0, 0).top_y, 100);
    }

    #[test]
    fn downsample_most_frequent_color_wins() {
        let mut m = flat(2, 2, 64);
        m.samples[0].color = 0xAAAA_AAAA;
        m.samples[1].color = 0xAAAA_AAAA;
        m.samples[2].color = 0xBBBB_BBBB;
        m.samples[3].color = 0xBBBB_BBBB;
        // 2 対 2 のタイは **後勝ち** (max_by_key は同値最大の最後を返す仕様。
        // 旧コメントの「先着」は実挙動と逆の虚偽だった。wave 101 DA-6 で訂正
        // し、確定的な後勝ちを厳密にピンする)。
        let d = downsample(&m);
        assert_eq!(d.get(0, 0).color, 0xBBBB_BBBB);
    }

    #[test]
    fn mesh_builds_quads_and_skirts() {
        let m = flat(2, 2, 64);
        let mesh = LodMesh::build(&m, 0, 1.0);
        assert_eq!(mesh.cell_size, 1);
        // 4 cells × (1 top quad + 4 skirt quads) = 4 × 5 = 20 quads = 40 tris
        assert_eq!(mesh.triangle_count(), 40);
        assert_eq!(mesh.vertices.len(), cells_check());
    }

    fn cells_check() -> usize {
        4 * 5 * 4
    }

    #[test]
    fn lod_distance_table() {
        assert_eq!(lod_for_distance(64.0), 0);
        assert_eq!(lod_for_distance(200.0), 1);
        assert_eq!(lod_for_distance(1500.0), 4);
    }
}

/// wave 101 (DA) で追加した厳密ピンテスト群。
/// 全ピン値は Python 厳密シミュレーション (角平均の窓/クランプ/複製重みまで
/// コードと同一論理) で機械確定したもの。
#[cfg(test)]
mod strict_tests {
    use super::*;

    fn map_const(w: u32, h: u32, samples: Vec<ColumnSample>) -> ColumnHeightmap {
        ColumnHeightmap {
            samples,
            width: w,
            height: h,
            origin_x: 0,
            origin_z: 0,
        }
    }

    fn solid(top_y: u16, color: u32) -> ColumnSample {
        ColumnSample {
            top_y,
            color,
            underwater: false,
        }
    }

    /// DA-1: 奇数寸法 (縁カラム静寂脱落) と構造不変量違反の fail-loud 化。
    #[test]
    fn downsample_rejects_odd_dims_and_bad_invariant() {
        let odd_w = map_const(3, 2, vec![solid(64, 1); 6]);
        assert!(
            std::panic::catch_unwind(|| downsample(&odd_w)).is_err(),
            "幅 3 (奇数) は panic 必須"
        );
        let odd_h = map_const(2, 3, vec![solid(64, 1); 6]);
        assert!(
            std::panic::catch_unwind(|| downsample(&odd_h)).is_err(),
            "高さ 3 (奇数) は panic 必須"
        );
        let bad = map_const(2, 2, vec![solid(64, 1); 3]); // len 3 != 2*2
        assert!(
            std::panic::catch_unwind(|| downsample(&bad)).is_err(),
            "構造不変量違反は panic 必須"
        );
        // 境界: 幅 1 は従来通り空マップを返す
        let thin = map_const(1, 2, vec![solid(64, 1); 2]);
        let d = downsample(&thin);
        assert_eq!(d.width, 0);
        assert!(d.samples.is_empty());
        // 2x2 -> 1x1 正規経路
        let ok = map_const(2, 2, vec![solid(64, 1); 4]);
        let d2 = downsample(&ok);
        assert_eq!((d2.width, d2.height), (1, 1));
        assert_eq!(d2.samples.len(), 1);
    }

    /// DA-2: 上面 quad の角高さスワップ根治の厳密ピン (4 セル全て)。
    /// old は [y00,y10,y11,y01] で、位置対応は [y00,y01,y11,y10] が数学的正解。
    #[test]
    fn top_quad_corner_heights_exact() {
        // 2x2 斜面: (0,0)=10 (1,0)=20 (0,1)=30 (1,1)=40 (samples は z*width+x)
        let m = map_const(
            2,
            2,
            vec![
                solid(10, 0xFF),
                solid(20, 0xFF),
                solid(30, 0xFF),
                solid(40, 0xFF),
            ],
        );
        let mesh = LodMesh::build(&m, 0, 1.0);
        // セル走査順 (z 外, x 内) × セルごと先頭 quad = 上面。
        // 日本語コメント: [x, y, z] (y は位置に対応した角平均, Python シム確定値)
        let expect: [[u16; 3]; 16] = [
            [0, 10, 0],
            [0, 20, 1],
            [1, 25, 1],
            [1, 15, 0], // cell(0,0): 旧版は [10,15,25,20] の誤
            [1, 15, 0],
            [1, 25, 1],
            [2, 30, 1],
            [2, 20, 0], // cell(1,0)
            [0, 20, 1],
            [0, 30, 2],
            [1, 35, 2],
            [1, 25, 1], // cell(0,1)
            [1, 25, 1],
            [1, 35, 2],
            [2, 40, 2],
            [2, 30, 1], // cell(1,1)
        ];
        for (k, exp) in expect.iter().enumerate() {
            let base = (k / 4) * 20 + (k % 4); // セルごと 20 verts, 上面 quad は先頭 4
            assert_eq!(
                mesh.vertices[base].pos_packed,
                *exp,
                "上面 quad 頂点 {k} (セル {})",
                k / 4
            );
        }
        assert_eq!(mesh.vertices.len(), 80);
        assert_eq!(mesh.triangle_count(), 40);
    }

    /// DA-5 + skirt 深度の量子化ピン: 12.5 は 13 (旧 trunc 12)、skirt 下端
    /// 8.5 は 9 (旧 trunc 8)。round は半を 0 から遠い側へ。
    /// 深度式 min(4, max(min_y,1)) のピンも兼ねる (min_y=1.0 → 深度 1.0:
    /// 本テスト初版は 4.0 達と誤解して赤を踏み self-capture 21 件目。
    /// コードは正しく、期待値の方を深度式に照らして訂正した経緯を残す)。
    #[test]
    fn quantize_rounds_to_nearest_half_away() {
        let m = map_const(2, 1, vec![solid(10, 0xFF), solid(15, 0xFF)]);
        // 上面: 角 (1,0),(1,1) の平均は 12.5 → 13 (min_y に依らず)
        let mesh = LodMesh::build(&m, 0, 4.0); // 深度 = min(4, max(4,1)) = 4.0
        assert_eq!(mesh.vertices[0].pos_packed, [0, 10, 0]);
        assert_eq!(mesh.vertices[1].pos_packed, [0, 10, 1]);
        assert_eq!(mesh.vertices[2].pos_packed, [1, 13, 1]);
        assert_eq!(mesh.vertices[3].pos_packed, [1, 13, 0]);
        // スカート北辺 (深度 4.0): 上端 (10, 12.5→13), 下端 (6.0→6, 8.5→9)
        assert_eq!(mesh.vertices[4].pos_packed, [0, 10, 0]);
        assert_eq!(mesh.vertices[5].pos_packed, [1, 13, 0]);
        assert_eq!(mesh.vertices[6].pos_packed, [1, 9, 0]);
        assert_eq!(mesh.vertices[7].pos_packed, [0, 6, 0]);
        // 深度式ピン: min_y = 1.0 → 深度 1.0 → 下端 (9.0→9, 11.5→12)
        let mesh1 = LodMesh::build(&m, 0, 1.0);
        assert_eq!(mesh1.vertices[6].pos_packed, [1, 12, 0]);
        assert_eq!(mesh1.vertices[7].pos_packed, [0, 9, 0]);
        // 深度式: min_y = 0.0 でも max(1) で 1.0 に引き上げ (同下端)
        let mesh0 = LodMesh::build(&m, 0, 0.0);
        assert_eq!(mesh0.vertices[6].pos_packed, [1, 12, 0]);
        // 深度式: min_y = 3.0 → 深度 3.0 → 下端 (7.0→7, 9.5→10)
        let mesh3 = LodMesh::build(&m, 0, 3.0);
        assert_eq!(mesh3.vertices[6].pos_packed, [1, 10, 0]);
        assert_eq!(mesh3.vertices[7].pos_packed, [0, 7, 0]);
    }

    /// DA-4: しきい値境界の厳密表 (全て「以上」で次段側) + 非有限/負の遮断。
    #[test]
    fn lod_distance_boundaries_exact() {
        for (d, expect) in [
            (0.0f32, 0u32),
            (127.5, 0),
            (128.0, 1),
            (255.5, 1),
            (256.0, 2),
            (511.5, 2),
            (512.0, 3),
            (1023.5, 3),
            (1024.0, 4),
            (2047.5, 4),
            (2048.0, 5),
            (1.0e6, 5),
        ] {
            assert_eq!(lod_for_distance(d), expect, "dist={d}");
        }
        for bad in [f32::NAN, -1.0, f32::NEG_INFINITY, f32::INFINITY] {
            assert!(
                std::panic::catch_unwind(|| lod_for_distance(bad)).is_err(),
                "dist={bad}: panic 必須 (旧実装は NaN/±inf が静寂に LOD 5 化)"
            );
        }
        // 現消費者経路は i32 hypot 由来で非発火を照合済 (render_pipeline:1016/1032/1050)
    }

    /// DA-6: merge_4 の確定的意味論 (タイ後勝ち / absent の色は不参加 / OR / max)。
    #[test]
    fn merge4_exact_semantics() {
        let at = |top_y: u16, color: u32, u: bool| ColumnSample {
            top_y,
            color,
            underwater: u,
        };
        // タイ 2-2 は後勝ち
        let tie = merge_4(
            at(64, 0xA, false),
            at(64, 0xA, false),
            at(64, 0xB, false),
            at(64, 0xB, false),
        );
        assert_eq!(tie.color, 0xB);
        // 過半数は 3-1 で先勝ち関係なく多数派
        let maj = merge_4(
            at(64, 0xA, false),
            at(64, 0xB, false),
            at(64, 0xA, false),
            at(64, 0xA, false),
        );
        assert_eq!(maj.color, 0xA);
        // 代表高さは absent 混じりでも max
        let mx = merge_4(
            at(0, 0xC, false),
            at(100, 0xA, false),
            at(64, 0xA, false),
            at(64, 0xA, false),
        );
        assert_eq!(mx.top_y, 100);
        // absent (top_y == 0) の色は最多頻度に参加しない
        let ex = merge_4(
            at(0, 0xC, false),
            at(64, 0xD, false),
            at(64, 0xD, false),
            at(64, 0xD, false),
        );
        assert_eq!(ex.color, 0xD);
        // underwater は OR
        let uw = merge_4(
            at(64, 1, false),
            at(64, 1, true),
            at(64, 1, false),
            at(64, 1, false),
        );
        assert!(uw.underwater);
        // 全 absent は air
        let air = merge_4(
            ColumnSample::air(),
            ColumnSample::air(),
            ColumnSample::air(),
            ColumnSample::air(),
        );
        assert_eq!((air.top_y, air.color, air.underwater), (0, 0, false));
    }

    /// DA-3: origin 非依存のローカル pack / extent・lod・不変量の fail-loud。
    #[test]
    fn build_extent_and_origin_contracts() {
        // origin != 0 でも pos_packed はローカル (旧実装は origin 分崩壊)
        let mut m = map_const(2, 2, vec![solid(64, 0xFF); 4]);
        m.origin_x = 16;
        m.origin_z = 32;
        let mesh = LodMesh::build(&m, 0, 1.0);
        assert_eq!(mesh.vertices[0].pos_packed, [0, 64, 0]);
        assert_eq!(mesh.vertices[3].pos_packed, [1, 64, 0]);
        // extent 超過: width=2, lod=15 → 2*32768 = 65536 > 65535
        let big = map_const(2, 2, vec![solid(64, 1); 4]);
        assert!(
            std::panic::catch_unwind(|| LodMesh::build(&big, 15, 1.0)).is_err(),
            "extent 超過は panic 必須"
        );
        // lod >= 16 はシフト前に拒否
        assert!(
            std::panic::catch_unwind(|| LodMesh::build(&big, 16, 1.0)).is_err(),
            "lod 16 は panic 必須"
        );
        // 構造不変量
        let bad = map_const(2, 2, vec![solid(64, 1); 5]);
        assert!(
            std::panic::catch_unwind(|| LodMesh::build(&bad, 0, 1.0)).is_err(),
            "不変量違反は panic 必須"
        );
        // 0 次元は空メッシュ
        let empty = ColumnHeightmap {
            samples: vec![],
            width: 0,
            height: 0,
            origin_x: 0,
            origin_z: 0,
        };
        let em = LodMesh::build(&empty, 0, 1.0);
        assert_eq!(em.vertices.len(), 0);
        assert_eq!(em.triangle_count(), 0);
    }
}
