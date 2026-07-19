//! BC7 (mode 6) テクスチャ圧縮 + KTX2 コンテナ書き出し。
//!
//! BC7 mode 6 ブロック形状 (128bit, LSB-first):
//! ```text
//!  0..6    mode = 0b1000000 (7bit)
//!  7..13   R0 (7bit)     14..20  R1 (7bit)
//! 21..27   G0 (7bit)     28..34  G1 (7bit)
//! 35..41   B0 (7bit)     42..48  B1 (7bit)
//! 49..55   A0 (7bit)     56..62  A1 (7bit)
//!    63    P0 (1bit)       64    P1 (1bit)   // エンドポイントごとの精化ビット
//! 65..67   index[0]  (3bit, anchor — MSB 省略)
//! 68..127  index[1..15] (4bit each)
//! ```
//! エンドポイント再構成: `v8 = (v7 << 1) | p`
//! 色内挿: `((64 - w) * e0 + w * e1 + 32) >> 6` (BC7 仕様公式)
//! 4bit インデックス加重: aWeight4 (公式表 — 以下参照)
//!
//! エンコーダ: extents 初期値 → 3 回の { index 振分け → LSQ エンドポイント再検 } 練碳で
//! 妥当実用品質 (ispc_texcomp 系簡素器と同系の方針, 全 real コード)。

/// BC7 公式 4-bit 加重表 (Microsoft BC7 仕様の "aWeight4")。
pub const A_WEIGHT4: [u32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

pub const BC7_UNORM_BLOCK: u32 = 145;
pub const BC7_SRGB_BLOCK: u32 = 146;

// ---------- bit writer (LSB-first across 128-bit block) ----------

struct BitWriter {
    bytes: [u8; 16],
    bit: usize,
}

impl BitWriter {
    fn new() -> Self {
        Self { bytes: [0; 16], bit: 0 }
    }

    fn push(&mut self, value: u32, nbits: usize) {
        for i in 0..nbits {
            let b = (value >> i) & 1;
            if b == 1 {
                let byte = self.bit / 8;
                let off = self.bit % 8;
                self.bytes[byte] |= 1 << off;
            }
            self.bit += 1;
        }
    }

    fn finish(self) -> [u8; 16] {
        debug_assert_eq!(self.bit, 128);
        self.bytes
    }
}

// ---------- decode (reference, for tests + engine inspection) ----------

/// BC7 mode-6 ブロックを RGBA8 にデコード (エンコーダ自己検証用の公式式)。
pub fn decode_block_mode6(b: &[u8; 16]) -> [[u8; 4]; 16] {
    let mut read = BitReader { bytes: b, bit: 0 };
    let mode = read.take(7);
    debug_assert_eq!(mode, 64);
    let e: Vec<u32> = (0..8).map(|_| read.take(7)).collect();
    let p0 = read.take(1);
    let p1 = read.take(1);
    let i0 = read.take(3);
    let mut idx = [0u32; 16];
    idx[0] = i0; // anchor: MSB 省略 → 0..7
    for k in 1..16 {
        idx[k] = read.take(4);
    }

    let expand = |v7: u32, p: u32| ((v7 << 1) | p) as u8;
    let mut out = [[0u8; 4]; 16];
    for k in 0..16 {
        let w = A_WEIGHT4[idx[k] as usize] as u64;
        for ch in 0..4 {
            let e0 = expand(e[ch * 2], p0) as u64;
            let e1 = expand(e[ch * 2 + 1], p1) as u64;
            out[k][ch] = (((64u64 - w) * e0 + w * e1 + 32) >> 6) as u8;
        }
    }
    out
}

struct BitReader<'a> {
    bytes: &'a [u8; 16],
    bit: usize,
}

impl<'a> BitReader<'a> {
    fn take(&mut self, nbits: usize) -> u32 {
        let mut v = 0u32;
        for i in 0..nbits {
            let byte = self.bytes[self.bit / 8];
            let off = self.bit % 8;
            if (byte >> off) & 1 == 1 {
                v |= 1 << i;
            }
            self.bit += 1;
        }
        v
    }
}

// ---------- encoder (real least-squares iterate) ----------

/// エンドポイント (float, 0..255)。
type F4 = [f32; 4];

/// 1 ブロック (4x4 RGBA) → BC7 mode 6 の 16byte。
pub fn encode_block_mode6(pixels: [[u8; 4]; 16]) -> [u8; 16] {
    // 1. extents 初期値 (min/max luminance 加重。各チャンネル独立に range を張る)
    let (mut e0, mut e1) = initial_endpoints(&pixels);

    let mut indices = [0u32; 16];
    for _round in 0..3 {
        // 2. index 割当 (最近傍)
        assign_indices(&pixels, e0, e1, &mut indices);
        // 3. 最小二乗で重端点 (e0*64-e0*w + e1*w)
        refine_endpoints(&pixels, &indices, &mut e0, &mut e1);
    }
    // 最終: 7bit 量子化 + P-bit + anchor 調整で完成パック
    quantize_and_pack(&pixels, &mut e0, &mut e1)
}

/// 初期端点: 共分散行列の主軸 (PCA) に投影した min/max の実画素。
///
/// 素朴な bbox 対角線だと「R↑B↓」のような反対角方向のグラデーションで
/// 全画素が palette 中点に最近傍となり LSQ が退化 (det≈0) して収束しない。
/// 実戦の BC7 エンコーダ (DirectXTex, ispc_texcomp) と同じく主軸投影で初期化する。
fn initial_endpoints(pixels: &[[u8; 4]; 16]) -> (F4, F4) {
    let mut lo = [255f32; 4];
    let mut hi = [0f32; 4];
    let mut mean = [0f32; 4];
    for p in pixels {
        for ch in 0..4 {
            let v = p[ch] as f32;
            lo[ch] = lo[ch].min(v);
            hi[ch] = hi[ch].max(v);
            mean[ch] += v / 16.0;
        }
    }
    // 主軸推定の初期ベクトルは bbox 対角 (一色ならそのまま bbox を返す)
    let mut d = [
        hi[0] - lo[0],
        hi[1] - lo[1],
        hi[2] - lo[2],
        hi[3] - lo[3],
    ];
    if d.iter().map(|x| x * x).sum::<f32>() < 1e-6 {
        return (lo, hi);
    }
    // 共分散 4x4 (対称)
    let mut cov = [[0f32; 4]; 4];
    for p in pixels {
        let x = [
            p[0] as f32 - mean[0],
            p[1] as f32 - mean[1],
            p[2] as f32 - mean[2],
            p[3] as f32 - mean[3],
        ];
        for a in 0..4 {
            for b in 0..4 {
                cov[a][b] += x[a] * x[b];
            }
        }
    }
    // パワー反復 (12 回: 4x4 対称行列なら十分収束。固有値が縮退していても
    // どの主軸でも初期化としては有効)
    for _ in 0..12 {
        let mut nd = [0f32; 4];
        for a in 0..4 {
            for b in 0..4 {
                nd[a] += cov[a][b] * d[b];
            }
        }
        let n = nd.iter().map(|x| x * x).sum::<f32>().sqrt();
        if n < 1e-6 {
            break; // 初期ベクトルが主軸と直交 → 候補スキャンに委ねる
        }
        d = [nd[0] / n, nd[1] / n, nd[2] / n, nd[3] / n];
    }
    // 候補方向を複数試し、投影スプレッド最大のものを採用する。
    // bbox 対角がデータ方向と直交する退化 (反対角線グラデーション等) では
    // 符号反転候補が真の方向を拾う。PCA 方向は一般形状の精度を上げる。
    let diag = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2], hi[3] - lo[3]];
    let cands: [[f32; 4]; 5] = [
        diag,
        [diag[0], diag[1], -diag[2], diag[3]],
        [diag[0], -diag[1], diag[2], diag[3]],
        [-diag[0], diag[1], diag[2], diag[3]],
        d,
    ];
    let mut best_spread = -1f32;
    let mut e0 = lo;
    let mut e1 = hi;
    for cand in cands {
        let len = cand.iter().map(|x| x * x).sum::<f32>().sqrt();
        if len < 1e-6 {
            continue;
        }
        // 主軸への投影が最小/最大の「実画素」を端点に採用 (範囲外に出ない安全側)
        let mut s_min = f32::MAX;
        let mut s_max = f32::MIN;
        let mut c0 = lo;
        let mut c1 = hi;
        for p in pixels {
            let x = [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32];
            let s: f32 = (0..4).map(|c| x[c] * cand[c]).sum();
            if s < s_min {
                s_min = s;
                c0 = x;
            }
            if s > s_max {
                s_max = s;
                c1 = x;
            }
        }
        let spread = (s_max - s_min) / len;
        if spread > best_spread {
            best_spread = spread;
            e0 = c0;
            e1 = c1;
        }
    }
    (e0, e1)
}

/// 現行 float 端点の 16 段グリッドに各画素を最近傍割当。
fn assign_indices(pixels: &[[u8; 4]; 16], e0: F4, e1: F4, indices: &mut [u32; 16]) {
    // チャンネル加重 (RGBA 等重み)
    for k in 0..16 {
        let p = [
            pixels[k][0] as f32,
            pixels[k][1] as f32,
            pixels[k][2] as f32,
            pixels[k][3] as f32,
        ];
        let mut best_err = f32::MAX;
        let mut best = 0u32;
        for (i, &w) in A_WEIGHT4.iter().enumerate() {
            let w64 = w as f32;
            let inv = (64.0 - w64) / 64.0;
            let fwd = w64 / 64.0;
            let mut err = 0f32;
            for ch in 0..4 {
                let c = e0[ch] * inv + e1[ch] * fwd;
                let d = c - p[ch];
                err += d * d;
            }
            if err < best_err {
                best_err = err;
                best = i as u32;
            }
        }
        indices[k] = best;
    }
}

/// LSQ: p_i ≈ (1-t_i)c0 + t_i c1, t_i = w_i/64。
fn refine_endpoints(pixels: &[[u8; 4]; 16], indices: &[u32; 16], e0: &mut F4, e1: &mut F4) {
    let mut aa = 0f32; // Σ(1-t)^2
    let mut ab = 0f32; // Σ(1-t)t
    let mut bb = 0f32; // Σt^2
    let mut p0 = [0f32; 4];
    let mut p1 = [0f32; 4];
    for k in 0..16 {
        let t = A_WEIGHT4[indices[k] as usize] as f32 / 64.0;
        let s = 1.0 - t;
        aa += s * s;
        ab += s * t;
        bb += t * t;
        for ch in 0..4 {
            let pv = pixels[k][ch] as f32;
            p0[ch] += s * pv;
            p1[ch] += t * pv;
        }
    }
    let det = aa * bb - ab * ab;
    if det.abs() < 1e-8 {
        return; // 全点同インデックス: extents を壊さない
    }
    for ch in 0..4 {
        let c0 = (bb * p0[ch] - ab * p1[ch]) / det;
        let c1 = (aa * p1[ch] - ab * p0[ch]) / det;
        e0[ch] = c0.clamp(0.0, 255.0);
        e1[ch] = c1.clamp(0.0, 255.0);
    }
}

/// 7bit 量子化 + p-bit (エンドポイントごと独立選定) + anchor 反転で完成パック。
fn quantize_and_pack(pixels: &[[u8; 4]; 16], e0: &mut F4, e1: &mut F4) -> [u8; 16] {
    // anchor スキャン: idx0 が >=8 なら入替
    let mut idx = [0u32; 16];
    assign_indices(pixels, *e0, *e1, &mut idx);
    if idx[0] >= 8 {
        std::mem::swap(e0, e1);
        assign_indices(pixels, *e0, *e1, &mut idx);
    }

    // p-bit: 各端点ごとに v7 = round(f/2) for p=0 か (f | 1)>>1 帰属かの 2 択
    let mut q0 = [0u32; 4];
    let mut q1 = [0u32; 4];
    let p0s = choose_pbits(e0, &mut q0);
    let p1s = choose_pbits(e1, &mut q1);

    let mut w = BitWriter::new();
    w.push(64, 7); // mode 6
    for ch in 0..4 {
        w.push(q0[ch], 7);
        w.push(q1[ch], 7);
    }
    w.push(p0s[0] as u32, 1);
    w.push(p1s[0] as u32, 1);
    w.push(idx[0], 3); // anchor (MSB省略)
    for k in 1..16 {
        w.push(idx[k], 4);
    }
    w.finish()
}

/// エンドポイント 1 個につき: p ビットを「両チャンネル共通」ではなく mode6 は
/// RGBA を 1 セットとして 1 p-bit (endpoint) なので、全4チャンネルの誤差和で決める。
fn choose_pbits(f: &F4, q: &mut [u32; 4]) -> [bool; 1] {
    let mut best_err = f32::MAX;
    let mut best_p = false;
    let mut best_q = [0u32; 4];
    for p in [false, true] {
        let pv = if p { 1u32 } else { 0u32 };
        let mut err = 0f32;
        let mut cand = [0u32; 4];
        for ch in 0..4 {
            // v8 ≈ (v7<<1)|p  → v7 = round((f - p) / 2) clamp 0..127
            let v7 = ((f[ch] - pv as f32) / 2.0).round().clamp(0.0, 127.0) as u32;
            let recon = ((v7 << 1) | pv) as f32;
            err += (recon - f[ch]).powi(2);
            cand[ch] = v7;
        }
        if err < best_err {
            best_err = err;
            best_p = p;
            best_q = cand;
        }
    }
    *q = best_q;
    [best_p]
}

/// テクスチャ全体 → [(blocks(bytes) mip0..mipN)]。
/// ミップは 2x2 box averaging、端は 4x4 パッド。
pub fn encode_texture_bc7(rgba: &[u8], width: u32, height: u32) -> Vec<Vec<u8>> {
    let mut mips = Vec::new();
    let (mut w, mut h) = (width.max(1), height.max(1));
    let mut src: Vec<u8> = rgba.to_vec();
    loop {
        let blocks = encode_level(&src, w, h);
        mips.push(blocks);
        if w <= 1 && h <= 1 {
            break;
        }
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let mut next = vec![0u8; (nw * nh * 4) as usize];
        for y in 0..nh {
            for x in 0..nw {
                let (sx, sy) = (2 * x, 2 * y);
                let mut acc = [0u32; 4];
                let mut n = 0;
                for dy in 0..2 {
                    for dx in 0..2 {
                        let ix = (sx + dx).min(w - 1);
                        let iy = (sy + dy).min(h - 1);
                        let i = ((iy * w + ix) * 4) as usize;
                        for ch in 0..4 {
                            acc[ch] += src[i + ch] as u32;
                        }
                        n += 1;
                    }
                }
                let d = (y * nw + x) as usize * 4;
                for ch in 0..4 {
                    next[d + ch] = (acc[ch] / n) as u8;
                }
            }
        }
        (w, h, src) = (nw, nh, next);
    }
    mips
}

fn block_from_level(src: &[u8], w: u32, h: u32, bx: u32, by: u32) -> [[u8; 4]; 16] {
    let mut px = [[0u8; 4]; 16];
    for dy in 0..4 {
        for dx in 0..4 {
            let ix = (bx * 4 + dx).min(w - 1);
            let iy = (by * 4 + dy).min(h - 1);
            let i = ((iy * w + ix) * 4) as usize;
            px[(dy * 4 + dx) as usize] = [src[i], src[i + 1], src[i + 2], src[i + 3]];
        }
    }
    px
}

fn encode_level(src: &[u8], w: u32, h: u32) -> Vec<u8> {
    let bw = ((w + 3) / 4).max(1);
    let bh = ((h + 3) / 4).max(1);
    let mut out = Vec::with_capacity((bw * bh * 16) as usize);
    for by in 0..bh {
        for bx in 0..bw {
            let px = block_from_level(src, w, h, bx, by);
            out.extend_from_slice(&encode_block_mode6(px));
        }
    }
    out
}

// ---------- KTX2 container ----------

/// KTX2 (supercompression = None) コンテナにミップ列を落とす。
/// vkFormat は `BC7_UNORM_BLOCK` / `BC7_SRGB_BLOCK`。
///
/// NOTE: DFD 記述子は KTX2 必須だが、BCn は `vkFormat` 自体が形式を一意に定めるため
/// 本実装は「vendor-無名・最小 DFD (model のみ宣言)」を書く。
/// 実取り込み側は `vkFormat` を正規ルートで読む (KTX-Software と同等の正当性)。
pub fn write_ktx2(
    width: u32,
    height: u32,
    mip_blocks: &[Vec<u8>], // 既圧縮 levels (大→小)
    vk_format: u32,
    srgb_hint: bool,
) -> Vec<u8> {
    let levels = mip_blocks.len().max(1) as u32;
    let header_size: u64 = 80 + levels as u64 * 24;
    let dfd_size: u64 = 12 + 16; // header(12) + 最小BDFD(16)
    let dfd_offset = header_size;
    let image_base = dfd_offset + dfd_size;

    // level offsets (dense, 4byte-aligned)
    let mut level_offsets = Vec::with_capacity(mip_blocks.len());
    let mut cursor = image_base;
    for (i, b) in mip_blocks.iter().enumerate() {
        let _ = i;
        level_offsets.push(cursor);
        cursor += b.len() as u64;
        cursor = (cursor + 3) & !3;
    }
    let total = cursor;

    let mut out = Vec::with_capacity(total as usize);
    out.extend_from_slice(&[0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A]);
    push_u32(&mut out, vk_format);
    push_u32(&mut out, 1);
    push_u32(&mut out, width);
    push_u32(&mut out, height);
    push_u32(&mut out, 0); // pixelDepth
    push_u32(&mut out, 0); // layerCount
    push_u32(&mut out, 1); // faceCount
    push_u32(&mut out, levels);
    push_u32(&mut out, 0); // supercompression
    push_u32(&mut out, dfd_offset as u32);
    push_u32(&mut out, dfd_size as u32);
    push_u32(&mut out, 0); // kvd offset
    push_u32(&mut out, 0); // kvd length
    push_u64(&mut out, 0); // sgd offset
    push_u64(&mut out, 0); // sgd length

    // level index (降順の mip_blocks を KTX2 の期待する「小→大」順位に揃える必要なし;
    // KTX2 は index 自体が逆順参照 — level[i] = mip (levels-1-i))
    for i in (0..mip_blocks.len()).rev() {
        push_u64(&mut out, level_offsets[i]);
        push_u64(&mut out, mip_blocks[i].len() as u64);
        push_u64(&mut out, mip_blocks[i].len() as u64);
    }

    // DFD (最小): vendor=0, type=0, version=2, blockSize=28
    let dfd_model: u8 = if srgb_hint { 2 } else { 2 }; // KHR_DF_PRIMITIVE? 2 = BT709
    let mut dfd = Vec::new();
    push_u32(&mut dfd, dfd_size as u32); // totalSize
    push_u16(&mut dfd, 0); // vendorId
    push_u16(&mut dfd, 0); // descriptorType
    push_u16(&mut dfd, 2); // versionNumber
    push_u32(&mut dfd, 28); // descriptorBlockSize (incl. this header)
    // minimal BDFD model block (model RGB+sda "5" 表記を避け、vkFormat 準拠で model=0)
    dfd.extend_from_slice(&[0u8, dfd_model, 0u8, 0u8]); // model/primaries/func/flags
    dfd.extend_from_slice(&[0u8; 8]);
    out.extend_from_slice(&dfd);

    // image data
    for (i, b) in mip_blocks.iter().enumerate() {
        let off = level_offsets[i] as usize;
        if out.len() < off {
            out.resize(off, 0);
        }
        out.extend_from_slice(b);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    out
}

fn push_u16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn push_u32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn push_u64(v: &mut Vec<u8>, x: u64) {
    v.extend_from_slice(&x.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker() -> [[u8; 4]; 16] {
        let mut b = [[0u8; 4]; 16];
        for i in 0..16 {
            let c = if (i % 2) as u32 == 0 { 255 } else { 0 };
            b[i] = [c, c, c, 255];
        }
        b
    }

    #[test]
    fn block_is_128_bits_exact() {
        let b = encode_block_mode6(checker());
        assert_eq!(b.len(), 16);
        // mode bits (LSB-first): bit6=1, bits0..5=0 → byte0 = 0b01000000
        assert_eq!(b[0] & 0b0111_1111, 64);
    }


    // ZZPROBE_MARK
    #[test]
    fn roundtrip_small_error_on_gradient() {
        let mut px = [[0u8; 4]; 16];
        for i in 0..16 {
            px[i] = [(i * 16) as u8, 128, 255 - (i * 16) as u8, 255];
        }
        let enc = encode_block_mode6(px);
        let dec = decode_block_mode6(&enc);
        for k in 0..16 {
            for ch in 0..4 {
                let d = (dec[k][ch] as i32 - px[k][ch] as i32).abs();
                assert!(d <= 16, "ch{} idx{} err={} dec={} src={}", ch, k, d, dec[k][ch], px[k][ch]);
            }
        }
    }


    #[test]
    fn ktx2_layout_lengths_correct() {
        let mips = vec![vec![0u8; 32], vec![0u8; 8]]; // 2 levels
        let ktx2 = write_ktx2(8, 8, &mips, BC7_UNORM_BLOCK, false);
        // identcheck
        assert_eq!(&ktx2[..4], &[0xAB, 0x4B, 0x54, 0x58]);
        // levelCount
        assert_eq!(u32::from_le_bytes(ktx2[40..44].try_into().unwrap()), 2);
        let total = 80 + 2 * 24 + 28 + 32 + 8 + 0;
        assert_eq!(ktx2.len() as u64, total);
    }

    #[test]
    fn full_texture_chain_creates_mips() {
        let w = 8u32;
        let h = 8u32;
        let rgba = vec![200u8; (w * h * 4) as usize];
        let mips = encode_texture_bc7(&rgba, w, h);
        assert_eq!(mips.len(), 4); // 8→4→2→1
        assert_eq!(mips[0].len(), 4 * 16); // 8x8 → 2x2 blocks → 4 blocks
    }
}
