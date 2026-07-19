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
        Self {
            layer_w,
            layer_h,
            layers: Vec::new(),
        }
    }

    pub fn push_layer_rgba(&mut self, pixels: Vec<u8>) -> Result<u32, String> {
        let expect = (self.layer_w * self.layer_h * 4) as usize;
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
pub fn generate_mip_chain(width: u32, height: u32, rgba: &[u8]) -> Vec<Vec<u8>> {
    let mut chain = Vec::new();
    let mut w = width;
    let mut h = height;
    let mut cur = rgba.to_vec();
    chain.push(cur.clone());
    while w > 1 || h > 1 {
        let nw = (w / 2).max(1);
        let nh = (h / 2).max(1);
        let mut next = vec![0u8; (nw * nh * 4) as usize];
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
        w = nw;
        h = nh;
        cur = next.clone();
        chain.push(next);
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
}
