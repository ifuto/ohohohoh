//! Texture atlas + texture array + mipmap chain (Tier 1).
//! Reduces bind changes and VRAM via shared atlases (Sodium/Iris pattern).

#[derive(Debug, Clone, Copy)]
pub struct AtlasRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone)]
pub struct PackedSprite {
    pub name_hash: u64,
    pub rect: AtlasRect,
    /// UV in 0..1 atlas space (min_u, min_v, max_u, max_v).
    pub uv: [f32; 4],
}

/// Shelf packer into a power-of-two atlas.
#[derive(Debug)]
pub struct TextureAtlasPacker {
    pub width: u32,
    pub height: u32,
    shelf_x: u32,
    shelf_y: u32,
    shelf_h: u32,
    pub sprites: Vec<PackedSprite>,
    failed: u32,
}

impl TextureAtlasPacker {
    pub fn new(width: u32, height: u32) -> Self {
        // 契約 (2026-07-22 wave 25 監査で追加): 各次元は <= 2^31。
        // これを超えると next_power_of_two が u32 で溢れ (debug panic /
        // release では 0 に wrap → .max(64) により「静かに 64x64 の
        // ミニアトラス」という fail-silent)。有一次元が超過する
        // スプライトは pack() 側で失敗カウントに回る既存仕様。
        assert!(
            width <= (1 << 31) && height <= (1 << 31),
            "TextureAtlasPacker::new: dims must be <= 2^31 (got {width}x{height})"
        );
        let w = width.next_power_of_two().max(64);
        let h = height.next_power_of_two().max(64);
        Self {
            width: w,
            height: h,
            shelf_x: 0,
            shelf_y: 0,
            shelf_h: 0,
            sprites: Vec::new(),
            failed: 0,
        }
    }

    /// シェルフ配置。同一の入力列には常に同一の配置 (整数演算のみで完全決定的、
    /// `shelf_layout_matches_hand_derived_geometry` テストが機械ピン)。
    /// None は (a) 0 次元 (b) atlas 寸法超過 (c) シェルフ残量不足 — いずれも
    /// `failed` カウントに累積する (種別は区別しない、監視用途)。
    pub fn pack(&mut self, name_hash: u64, w: u32, h: u32) -> Option<PackedSprite> {
        if w == 0 || h == 0 || w > self.width || h > self.height {
            self.failed += 1;
            return None;
        }
        if self.shelf_x + w > self.width {
            self.shelf_y += self.shelf_h;
            self.shelf_x = 0;
            self.shelf_h = 0;
        }
        if self.shelf_y + h > self.height {
            self.failed += 1;
            return None;
        }
        let rect = AtlasRect {
            x: self.shelf_x,
            y: self.shelf_y,
            w,
            h,
        };
        self.shelf_x += w;
        self.shelf_h = self.shelf_h.max(h);
        let uv = [
            rect.x as f32 / self.width as f32,
            rect.y as f32 / self.height as f32,
            (rect.x + rect.w) as f32 / self.width as f32,
            (rect.y + rect.h) as f32 / self.height as f32,
        ];
        let sprite = PackedSprite {
            name_hash,
            rect,
            uv,
        };
        self.sprites.push(sprite.clone());
        Some(sprite)
    }

    pub fn vram_bytes_rgba8(&self) -> u64 {
        let base = self.width as u64 * self.height as u64 * 4;
        // + mip chain ≈ +1/3
        base + base / 3
    }

    pub fn failed_count(&self) -> u32 {
        self.failed
    }
}

/// Texture2DArray builder — one layer per sprite (no atlas bleed).
#[derive(Debug, Default)]
pub struct TextureArrayBuilder {
    pub layer_w: u32,
    pub layer_h: u32,
    pub layers: Vec<Vec<u8>>, // RGBA8
}

impl TextureArrayBuilder {
    pub fn new(layer_w: u32, layer_h: u32) -> Self {
        // 契約 (wave 25): 0 次元を拒否、かつ layer_w*layer_h*4 は u32 収容必須。
        // 旧実装は push 時の `w*h*4` を u32 で掛けており、65536x65536 (=2^34) で
        // wrap 後の expect=0 が「空ベクタを合法レイヤとして受理」するバグ経路
        // だった (debug では overflow panic、release では静寂)。
        let bytes = layer_w as u64 * layer_h as u64 * 4;
        assert!(
            layer_w > 0 && layer_h > 0 && bytes <= u32::MAX as u64,
            "TextureArrayBuilder::new: need 0 < dims and w*h*4 <= u32::MAX (got {layer_w}x{layer_h})"
        );
        Self {
            layer_w,
            layer_h,
            layers: Vec::new(),
        }
    }

    pub fn push_layer_rgba(&mut self, pixels: Vec<u8>) -> Result<u32, String> {
        let expect = (self.layer_w as u64 * self.layer_h as u64 * 4) as usize;
        if pixels.len() != expect {
            return Err(format!("layer size {} != {}", pixels.len(), expect));
        }
        let id = self.layers.len() as u32;
        self.layers.push(pixels);
        Ok(id)
    }

    pub fn vram_bytes(&self) -> u64 {
        let one = self.layer_w as u64 * self.layer_h as u64 * 4;
        let mips = one + one / 3;
        mips * self.layers.len() as u64
    }
}

/// Box-filter mipmap downsample (RGBA8).
/// 2x2 box を整数加算 + 切捨て除算で評価。縮退次元 (1xN / Nx1) は端クランプで
/// サンプルを複製して 4 サンプル一定に保つ (n は常に 4)。
/// 全レベルは入力に対し完全決定的 (整数演算のみ) —
/// `mip_chain_exact_bytes` が内容をバイトピン。
/// 契約 (wave 25): 0 次元拒否、かつ rgba.len() == w*h*4 必須
/// (旧実装は短小入力で途中 panic、超過入力で静かに無視していた)。
/// 内部では全レベルぶんの余分な clone を行わない
/// (旧実装は base + 各レベルで 1 回ずつ多重複製していた)。
pub fn generate_mip_chain(width: u32, height: u32, rgba: &[u8]) -> Vec<Vec<u8>> {
    assert!(
        width > 0 && height > 0 && rgba.len() as u64 == width as u64 * height as u64 * 4,
        "generate_mip_chain: dims {width}x{height} must match {} RGBA8 bytes",
        rgba.len()
    );
    let mut chain = Vec::with_capacity(16);
    chain.push(rgba.to_vec());
    let mut w = width;
    let mut h = height;
    while w > 1 || h > 1 {
        let nw = (w / 2).max(1);
        let nh = (h / 2).max(1);
        let mut next = vec![0u8; (nw * nh * 4) as usize];
        {
            let cur = &chain[chain.len() - 1];
            for y in 0..nh {
                for x in 0..nw {
                    let mut acc = [0u32; 4];
                    let mut n = 0u32;
                    for oy in 0..2 {
                        for ox in 0..2 {
                            let sx = (x * 2 + ox).min(w - 1);
                            let sy = (y * 2 + oy).min(h - 1);
                            let i = ((sy * w + sx) * 4) as usize;
                            for c in 0..4 {
                                acc[c] += cur[i + c] as u32;
                            }
                            n += 1;
                        }
                    }
                    let di = ((y * nw + x) * 4) as usize;
                    for c in 0..4 {
                        next[di + c] = (acc[c] / n) as u8;
                    }
                }
            }
        }
        chain.push(next);
        w = nw;
        h = nh;
    }
    chain
}

/// Remap local sprite UV (0..1) into atlas UV.
#[inline]
pub fn remap_uv(atlas_uv: [f32; 4], local_u: f32, local_v: f32) -> (f32, f32) {
    let u = atlas_uv[0] + (atlas_uv[2] - atlas_uv[0]) * local_u;
    let v = atlas_uv[1] + (atlas_uv[3] - atlas_uv[1]) * local_v;
    (u, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_and_mip() {
        let mut atlas = TextureAtlasPacker::new(128, 128);
        assert!(atlas.pack(1, 16, 16).is_some());
        assert!(atlas.pack(2, 32, 16).is_some());
        let rgba = vec![128u8; 16 * 16 * 4];
        let mips = generate_mip_chain(16, 16, &rgba);
        assert!(mips.len() >= 5);
        assert_eq!(mips.last().unwrap().len(), 4);
    }

    /// wave 25-1: シェルフ配置が手計算期待と完全一致すること (整数演算のみの
    /// 完全決定列)。A..G の配置・UV・failed 集計を一括ピン。
    /// shelf 状態遷移 (手追跡): A(0,0; x=16,h=16) B(16,0; x=48) C: 48+96>128
    /// 故に y=16,x=0 → (0,16; x=96) D: 96+40>128 故 y=32 → (0,32; x=40,h=8)
    /// E: 200>128 早期失敗 f=1 F: 40+128>128 故 y=40,x=0,h=0 でも 40+128>128
    /// で失敗 f=2 (状態進行は残る) G: 0+88<=128 → (0,40; x=88,h=8) 成功 f=2。
    #[test]
    fn shelf_layout_matches_hand_derived_geometry() {
        let mut a = TextureAtlasPacker::new(128, 128);
        let pa = a.pack(1, 16, 16).unwrap();
        let pb = a.pack(2, 32, 8).unwrap();
        let pc = a.pack(3, 96, 16).unwrap();
        let pd = a.pack(4, 40, 8).unwrap();
        assert!(a.pack(5, 200, 8).is_none(), "oversize must fail");
        assert!(a.pack(6, 128, 128).is_none(), "no room after D + advance");
        let pg = a.pack(7, 88, 8).unwrap();
        assert_eq!(a.failed_count(), 2);

        let packed = [pa, pb, pc, pd, pg];
        let rects: Vec<(u32, u32, u32, u32)> = packed
            .iter()
            .map(|p| (p.rect.x, p.rect.y, p.rect.w, p.rect.h))
            .collect();
        assert_eq!(
            rects,
            vec![
                (0, 0, 16, 16),
                (16, 0, 32, 8),
                (0, 16, 96, 16),
                (0, 32, 40, 8),
                (0, 40, 88, 8)
            ]
        );
        // UV は 2^7 除算で全て厳密 (丸め誤差ゼロ) — ビットピン可能
        let want_uv: [[f32; 4]; 5] = [
            [0.0, 0.0, 16.0 / 128.0, 16.0 / 128.0],
            [0.125, 0.0, 48.0 / 128.0, 8.0 / 128.0],
            [0.0, 0.125, 96.0 / 128.0, 32.0 / 128.0],
            [0.0, 0.25, 40.0 / 128.0, 40.0 / 128.0],
            [0.0, 40.0 / 128.0, 88.0 / 128.0, 48.0 / 128.0],
        ];
        for (i, p) in packed.iter().enumerate() {
            for c in 0..4 {
                assert_eq!(p.uv[c].to_bits(), want_uv[i][c].to_bits(), "uv[{i}][{c}]");
            }
        }
        // 矩形は相互に非重複 (シェルフ構造の最小不変条件)
        for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                let (ax, ay, aw, ah) = rects[i];
                let (bx, by, bw, bh) = rects[j];
                let overlap = ax < bx + bw && bx < ax + aw && ay < by + bh && by < ay + ah;
                assert!(!overlap, "rects {i} and {j} overlap");
            }
        }
    }

    /// wave 25-2: mip 内容のバイトピン (整数演算のみで完全決定、独立整数
    /// 参照で厳密導出)。4x4 → 2x2 → 1x1 の全バイトとレベル数、そして
    /// 縮退次元 (1x3 列: 端クランプが実際に発火する形状) の複製規則を固定。
    #[test]
    fn mip_chain_exact_bytes() {
        let mut rgba = Vec::new();
        for y in 0u8..4 {
            for x in 0u8..4 {
                rgba.extend_from_slice(&[
                    x * 60 + y,
                    y * 70 + x,
                    255 - x * 10 - y * 5,
                    100 + x + y,
                ]);
            }
        }
        let chain = generate_mip_chain(4, 4, &rgba);
        assert_eq!(chain.len(), 3);
        assert_eq!(
            chain[1],
            vec![30, 35, 247, 101, 150, 37, 227, 103, 32, 175, 237, 103, 152, 177, 217, 105],
            "2x2 level"
        );
        assert_eq!(chain[2], vec![91, 106, 232, 103], "1x1 level");
        // 1x3 列 → 1x1: min(w-1) クランプで単一列を複製、4 サンプル平均
        let col = vec![10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120];
        let chain = generate_mip_chain(1, 3, &col);
        assert_eq!(chain.len(), 2, "1x3 -> 1x1 direct");
        assert_eq!(chain[1], vec![30, 40, 50, 60], "clamped column average");
    }

    /// wave 25-3: 次元/サイズ契約違反は fail-loud (旧: u32 wrap で
    /// 空レイヤ受理 / 短小バッファで途中 panic / 2^31 超過で静かに 64x64 化)。
    #[test]
    #[should_panic(expected = "dims must be <= 2^31")]
    fn packer_rejects_dims_over_2pow31() {
        let _ = TextureAtlasPacker::new((1 << 31) + 1, 64);
    }

    #[test]
    #[should_panic(expected = "need 0 < dims and w*h*4 <= u32::MAX")]
    fn array_builder_rejects_overflowing_dims() {
        let _ = TextureArrayBuilder::new(65536, 65536);
    }

    #[test]
    #[should_panic(expected = "need 0 < dims and w*h*4 <= u32::MAX")]
    fn array_builder_rejects_zero_dims() {
        let _ = TextureArrayBuilder::new(0, 64);
    }

    #[test]
    #[should_panic(expected = "must match")]
    fn mip_chain_rejects_mismatched_buffer() {
        let _ = generate_mip_chain(4, 4, &vec![0u8; 63]);
    }

    #[test]
    #[should_panic(expected = "must match")]
    fn mip_chain_rejects_zero_dims() {
        let _ = generate_mip_chain(0, 4, &[]);
    }
}
