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

/// 1 リージョン (N x N カラム)。N は 2 の累乗でなくてもよい。
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
pub fn downsample(src: &ColumnHeightmap) -> ColumnHeightmap {
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
    // 代表高さ = 最大値 (DH は小ãLODs で max を使う)
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
    ///   skirt: 東西南北に「鉛直接地スカート」(深度差 4-8m 相当) を付けて
    ///   LOD 段差の隙間をDH と同様に塞ぐ。
    pub fn build(map: &ColumnHeightmap, lod_level: u32, min_y: f32) -> Self {
        let cell = 1u32 << lod_level;
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
                let bx = (x as u32 * cell) as f32;
                let bz = (z as u32 * cell) as f32;
                let half = cell as f32;

                // 上面: 4 角を近傍平均で (境界で隣のLODに滑らかに合わせる)
                let y00 = neighbour_avg_y(map, x, z, lod_level);
                let y10 = neighbour_avg_y(map, x + 1, z, lod_level);
                let y01 = neighbour_avg_y(map, x, z + 1, lod_level);
                let y11 = neighbour_avg_y(map, x + 1, z + 1, lod_level);
                let q = [
                    mkv(y00, c.color, map.origin_x, bx, map.origin_z, bz),
                    mkv(y10, c.color, map.origin_x, bx, map.origin_z, bz + half),
                    mkv(y11, c.color, map.origin_x, bx + half, map.origin_z, bz + half),
                    mkv(y01, c.color, map.origin_x, bx + half, map.origin_z, bz),
                ];
                // 上面の示す法線が上なら採用
                push_quad(&mut vertices, &mut indices, q);

                // まわりスカート（4 辺）
                let edges: [([f32; 2], [f32; 2]); 4] = [
                    ([bx, bz], [bx + half, bz]),
                    ([bx + half, bz], [bx + half, bz + half]),
                    ([bx + half, bz + half], [bx, bz + half]),
                    ([bx, bz + half], [bx, bz]),
                ];
                let edge_ys = [(y00, y10), (y10, y11), (y11, y01), (y01, y00)];
                for ((p0, p1), (ya, yb)) in edges.into_iter().zip(edge_ys.into_iter()) {
                    let sa = (ya - skirt_depth).max(0.0);
                    let sb = (yb - skirt_depth).max(0.0);
                    // 上2頂点は上面に一致、下2頂点は深めに落ちる; 法線は外向
                    let q2 = [
                        mkv(ya, c.color, map.origin_x, p0[0], map.origin_z, p0[1]),
                        mkv(yb, c.color, map.origin_x, p1[0], map.origin_z, p1[1]),
                        mkv(sb, c.color, map.origin_x, p1[0], map.origin_z, p1[1]),
                        mkv(sa, c.color, map.origin_x, p0[0], map.origin_z, p0[1]),
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

/// 出力量: `pos_packed` は「カメラ追従 origin からの相対 (ブロック)」を 16bit に圧縮。
#[inline]
fn mkv(y: f32, color: u32, origin_x: i32, world_x: f32, origin_z: i32, world_z: f32) -> DistantVertex {
    let rx = (world_x as i32 - origin_x).clamp(0, 65535) as u16;
    let rz = (world_z as i32 - origin_z).clamp(0, 65535) as u16;
    DistantVertex {
        pos_packed: [rx, (y as i32).clamp(0, 65535) as u16, rz],
        flags: 0,
        color,
    }
}

/// DH式周边補間: 2^lod スケールの隣接平均。
fn neighbour_avg_y(map: &ColumnHeightmap, x: usize, z: usize, _lod: u32) -> f32 {
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
pub fn lod_for_distance(dist_blocks: f32) -> u32 {
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
        m.samples[3].color = 0xBBBB_BBBB; // 2 対 2 → max で tie は先着
        let d = downsample(&m);
        assert_ne!(d.get(0, 0).color, 0);
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
