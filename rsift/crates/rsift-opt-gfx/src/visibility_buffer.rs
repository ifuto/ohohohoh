//! Visibility Buffer — store primitive + instance IDs instead of a fat G-buffer.
//!
//! A visibility buffer keeps only a per-pixel `(primitive_id, instance_id)`
//! (typically 8 bytes in 64-bit mode) in the forward pass; material/attributes are
//! fetched and shaded via bindless vertex pulling (`StorageBuffer<Vertex>`) in a
//! second pass. Slashes G-buffer VRAM bandwidth on tile-based GPUs from ~32B to ~8B.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec4 {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Vec4 {
    #[inline]
    pub fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }
}

/// Pack `(primitive_id, instance_id)` into one `u32`: primitive in the low 16
/// bits, instance in the high 16 bits.
///
/// **契約 (wave 79 CC-1 で fail-loud 化)**: 両入力は `< 65536` 必須。
/// 旧実装は `& 0xFFFF` で上位ビットを**静寂切捨て**し、異なる ID が同じ
/// パック値へエイリアスした (AV-1 pack_handle・BE-1 new() と同一クラスの
/// ハザード)。65536 以上の ID が必要な領域は [`pack_ids_64`] を使うこと。
#[inline]
pub fn pack_ids(primitive: u32, instance: u32) -> u32 {
    assert!(
        primitive < 65536 && instance < 65536,
        "pack_ids 契約違反: primitive={primitive}, instance={instance} — \
         ≥65536 は 16bit 欄への静寂切捨て (ID エイリアス)。pack_ids_64 を使用"
    );
    (primitive & 0xFFFF) | ((instance & 0xFFFF) << 16)
}

/// Inverse of [`pack_ids`].
#[inline]
pub fn unpack_ids(packed: u32) -> (u32, u32) {
    (packed & 0xFFFF, (packed >> 16) & 0xFFFF)
}

/// Full 64-bit visibility buffer packing (32-bit primitive ID + 32-bit instance ID)
/// for large open-world Minecraft chunks (>65,536 triangles or instances).
#[inline]
pub fn pack_ids_64(primitive: u32, instance: u32) -> u64 {
    (primitive as u64) | ((instance as u64) << 32)
}

/// Inverse of [`pack_ids_64`].
#[inline]
pub fn unpack_ids_64(packed: u64) -> (u32, u32) {
    (packed as u32, (packed >> 32) as u32)
}

/// Visibility Buffer リゾルバ — GPU ラスタが書いた ID バッファ
/// (1px = [`pack_ids`] パック値) を CPU リードバック後に解釈する実データ集計器。
/// Aokana の可視リージョン判定や Hi-Z カリングの実測検証に使用する。
#[derive(Debug, Clone, Copy)]
pub struct VisibilityBufferResolver {
    pub width: u32,
    pub height: u32,
}

impl VisibilityBufferResolver {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// ID バッファの全サンプルを展開し、instance 毎の出現ピクセル数を数える。
    /// 「0 番 = 背景」等の規約は呼び出し側のラスタ設定に委ね、ここでは
    /// 実データの集計のみを行う。`id_buffer` が画面より短い場合は実長で打ち切る。
    pub fn histogram_instances(&self, id_buffer: &[u32]) -> std::collections::HashMap<u32, u32> {
        let max = (self.width as usize) * (self.height as usize);
        let mut hist = std::collections::HashMap::new();
        for &packed in id_buffer.iter().take(max) {
            let (_primitive, instance) = unpack_ids(packed);
            *hist.entry(instance).or_insert(0) += 1;
        }
        hist
    }
}

/// Reconstruct a fragment attribute by interpolating barycentric weights across
/// the primitive's 3 vertices (referenced by `primitive_id`). `w` are the three
/// barycentric weights.
#[inline]
pub fn interpolate(a: f32, b: f32, c: f32, w: [f32; 3]) -> f32 {
    a * w[0] + b * w[1] + c * w[2]
}

#[inline]
pub fn interpolate_rgb(a: Vec4, b: Vec4, c: Vec4, w: [f32; 3]) -> Vec4 {
    Vec4 {
        r: interpolate(a.r, b.r, c.r, w),
        g: interpolate(a.g, b.g, c.g, w),
        b: interpolate(a.b, b.b, c.b, w),
        a: interpolate(a.a, b.a, c.a, w),
    }
}

/// Compute barycentric weights $(u, v, w)$ given screen-space coordinates and triangle NDC/screen vertices.
///
/// 縮退扱い (wave 79 CC-2 で誠実化): `denom` は符号付き面積の 2 倍
/// (pixel² 単位)。`|denom| < 1e-6` (絶対閾値) の縮退三角形では、拒否ではなく
/// **重心重み [1/3, 1/3, 1/3] への定義済みフォールバック**を返す
/// (面積を持たないプリミティブのラスタで属性値を出すための規約。
/// 旧 doc はこの分岐を未記載だった)。
#[inline]
pub fn compute_barycentrics(
    px: f32,
    py: f32,
    v0: [f32; 2],
    v1: [f32; 2],
    v2: [f32; 2],
) -> [f32; 3] {
    let denom = (v1[1] - v2[1]) * (v0[0] - v2[0]) + (v2[0] - v1[0]) * (v0[1] - v2[1]);
    if denom.abs() < 1e-6 {
        return [0.3333333, 0.3333333, 0.3333333];
    }
    let inv = 1.0 / denom;
    let w0 = ((v1[1] - v2[1]) * (px - v2[0]) + (v2[0] - v1[0]) * (py - v2[1])) * inv;
    let w1 = ((v2[1] - v0[1]) * (px - v2[0]) + (v0[0] - v2[0]) * (py - v2[1])) * inv;
    let w2 = 1.0 - w0 - w1;
    [w0, w1, w2]
}

/// 12B 量子化頂点のワードレイアウト (CPU ミラーと共有する唯一の真実)。
/// `Quantized12ByteVertex` (pos_xyz u16[3] + oct_normal u8[2] + uv u16[2])
/// を little-endian の u32 ワード 3 個として読む:
/// - word0 = pos_x | pos_y<<16
/// - word1 = pos_z | (oct_x | oct_y<<8)<<16
/// - word2 = uv_u | uv_v<<16
pub const WORDS_PER_VERTEX: u32 = 3;

/// WGSL `decode_vertex` の CPU ミラー (3 連鎖語彙ピン対象)。
/// 位置の除数 1024.0 は `Quantized12ByteVertex::encode` の厳密ミラー
/// (16bit 固定小数点)。`words` は [`WORDS_PER_VERTEX`] 個の頂点ワード。
#[inline]
pub fn decode_pos(words: &[u32; 3]) -> [f32; 3] {
    [
        (words[0] & 0xFFFF) as f32 / 1024.0,
        ((words[0] >> 16) & 0xFFFF) as f32 / 1024.0,
        (words[1] & 0xFFFF) as f32 / 1024.0,
    ]
}

/// CPU ミラー: UV の除数 32767.0 (encode の u16 量子化スケールと厳密一致)。
#[inline]
pub fn decode_uv(words: &[u32; 3]) -> [f32; 2] {
    [
        (words[2] & 0xFFFF) as f32 / 32767.0,
        ((words[2] >> 16) & 0xFFFF) as f32 / 32767.0,
    ]
}

/// CPU ミラー: octahedral normal の decode (encode の L1 折り畳みの厳密な逆)。
/// 返り値は encode 入力の **L1 正規化点** (方向は一致、ノルムは L1 球面上)。
/// `z < 0` の折り返し解除は decode 標準形: 両成分は折り畳み前の x,y を
/// 厳密に復元する (考査: wave 79 CC-3 で往復解析済み、隅は f32 厳密)。
#[inline]
pub fn decode_normal_oct(oct_x: u8, oct_y: u8) -> [f32; 3] {
    let ox = oct_x as f32 / 127.5 - 1.0;
    let oy = oct_y as f32 / 127.5 - 1.0;
    let mut nx = ox;
    let mut ny = oy;
    let nz = 1.0 - ox.abs() - oy.abs();
    if nz < 0.0 {
        let sx = if nx >= 0.0 { 1.0 } else { -1.0 };
        let sy = if ny >= 0.0 { 1.0 } else { -1.0 };
        let tx = (1.0 - ny.abs()) * sx;
        let ty = (1.0 - nx.abs()) * sy;
        nx = tx;
        ny = ty;
    }
    [nx, ny, nz]
}

/// Generates WGSL for bindless vertex pulling + visibility resolve.
///
/// **wave 79 CC-3 (スタブ完全実装)**: 旧生成物は `pos_packed/uv_packed/
/// color_packed` の 3×u32・10bit pos という**コードベースに存在しない虚構
/// レイアウト**で、uv/color は未算出・visibility_texture と max_instances は
/// 未使用のスタブだった。本版は真の 12B レイアウト ([`WORDS_PER_VERTEX`])
/// に忠実な完全 resolve (pos/uv/normal decode + bary 補間 + visibility
/// 参照 + 範囲契約) を生成し、除数・語彙は CPU ミラー (`decode_pos`/
/// `decode_uv`/`decode_normal_oct`) と一致する (3 連鎖)。
///
/// `max_instances` は生成 WGSL の `MAX_INSTANCES` 定数として埋め込まれ、
/// resolve は `instance >= MAX_INSTANCES` を背景として discard する
/// (「0 番背景」規約は呼出側ラスタ設定、範囲外は描画しない契約)。
pub fn generate_bindless_pulling_wgsl(max_instances: usize) -> String {
    format!(
        r#"// Rsift visibility-buffer resolve (GENERATED). Mirrors Quantized12ByteVertex
// as 3 words/vertex: w0 = pos_x|pos_y<<16, w1 = pos_z|(oct_x|oct_y<<8)<<16,
// w2 = uv_u|uv_v<<16. Divisors 1024.0 / 32767.0 / 127.5 match the CPU mirror.
const MAX_INSTANCES: u32 = {max_instances}u;
const WORDS_PER_VERTEX: u32 = 3u;

struct PulledFragment {{
    pos: vec3<f32>,
    normal: vec3<f32>,
    uv: vec2<f32>,
}};

@group(0) @binding(0) var<storage, read> vertex_words: array<u32>;
@group(0) @binding(1) var<storage, read> index_pool: array<u32>;
@group(0) @binding(2) var visibility_tex: texture_2d<u32>;
@group(0) @binding(3) var bary_tex: texture_2d<f32>;

fn decode_vertex(word_off: u32) -> PulledFragment {{
    let w0 = vertex_words[word_off];
    let w1 = vertex_words[word_off + 1u];
    let w2 = vertex_words[word_off + 2u];
    var f: PulledFragment;
    f.pos = vec3<f32>(
        f32(w0 & 0xFFFFu) / 1024.0,
        f32((w0 >> 16u) & 0xFFFFu) / 1024.0,
        f32(w1 & 0xFFFFu) / 1024.0,
    );
    let ox = f32((w1 >> 16u) & 0xFFu) / 127.5 - 1.0;
    let oy = f32((w1 >> 24u) & 0xFFu) / 127.5 - 1.0;
    var n = vec3<f32>(ox, oy, 1.0 - abs(ox) - abs(oy));
    if n.z < 0.0 {{
        let sx = select(-1.0, 1.0, n.x >= 0.0);
        let sy = select(-1.0, 1.0, n.y >= 0.0);
        let tx = (1.0 - abs(n.y)) * sx;
        n = vec3<f32>(tx, (1.0 - abs(n.x)) * sy, n.z);
    }}
    f.normal = n;
    f.uv = vec2<f32>(
        f32(w2 & 0xFFFFu) / 32767.0,
        f32((w2 >> 16u) & 0xFFFFu) / 32767.0,
    );
    return f;
}}

fn pull_and_decode(prim_id: u32, bary: vec3<f32>) -> PulledFragment {{
    let base_idx = prim_id * 3u;
    let v0 = decode_vertex(index_pool[base_idx] * WORDS_PER_VERTEX);
    let v1 = decode_vertex(index_pool[base_idx + 1u] * WORDS_PER_VERTEX);
    let v2 = decode_vertex(index_pool[base_idx + 2u] * WORDS_PER_VERTEX);
    var r: PulledFragment;
    r.pos = v0.pos * bary.x + v1.pos * bary.y + v2.pos * bary.z;
    r.normal = v0.normal * bary.x + v1.normal * bary.y + v2.normal * bary.z;
    r.uv = v0.uv * bary.x + v1.uv * bary.y + v2.uv * bary.z;
    return r;
}}

@fragment
fn resolve_main(@builtin(position) fc: vec4<f32>) -> @location(0) vec4<f32> {{
    let pix = vec2<i32>(i32(fc.x), i32(fc.y));
    let packed = textureLoad(visibility_tex, pix, 0).r;
    let prim = packed & 0xFFFFu;
    let inst = (packed >> 16u) & 0xFFFFu;
    if prim == 0xFFFFu || inst >= MAX_INSTANCES {{
        discard;
    }}
    let bary = textureLoad(bary_tex, pix, 0).rgb;
    let frag = pull_and_decode(prim, bary);
    return vec4<f32>(frag.uv, 0.0, 1.0);
}}
"#
    )
}

pub fn visibility_buffer_wgsl() -> &'static str {
    VISIBILITY_BUFFER_WGSL
}

pub const VISIBILITY_BUFFER_WGSL: &str = include_str!("../shaders/visibility_buffer.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_roundtrip() {
        let p = pack_ids(0x1234, 0xABCD);
        assert_eq!(p & 0xFFFF, 0x1234);
        assert_eq!((p >> 16) & 0xFFFF, 0xABCD);
        assert_eq!(unpack_ids(p), (0x1234, 0xABCD));
    }

    #[test]
    fn pack_64_roundtrip() {
        let p = pack_ids_64(0x12345678, 0x87654321);
        assert_eq!(unpack_ids_64(p), (0x12345678, 0x87654321));
    }

    #[test]
    fn interpolate_at_vertices() {
        assert!((interpolate(2.0, 5.0, 9.0, [1.0, 0.0, 0.0]) - 2.0).abs() < 1e-6);
        assert!((interpolate(2.0, 5.0, 9.0, [0.0, 0.0, 1.0]) - 9.0).abs() < 1e-6);
    }

    #[test]
    fn interpolate_rgb_in_range() {
        let r = interpolate_rgb(
            Vec4::new(1.0, 0.0, 0.0, 1.0),
            Vec4::new(0.0, 1.0, 0.0, 1.0),
            Vec4::new(0.0, 0.0, 1.0, 1.0),
            [0.5, 0.5, 0.0],
        );
        assert!((r.r - 0.5).abs() < 1e-6);
        assert!((r.g - 0.5).abs() < 1e-6);
        assert!((r.b - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_barycentric_computation() {
        let bary = compute_barycentrics(5.0, 5.0, [0.0, 0.0], [10.0, 0.0], [0.0, 10.0]);
        assert!((bary[0] + bary[1] + bary[2] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_wgsl_gen() {
        let code = generate_bindless_pulling_wgsl(4096);
        assert!(code.contains("pull_and_decode"));
        assert!(code.contains("resolve_main"));
        // CC-3: 旧スタブの虚構レイアウト語彙は完全消去 (実 12B 語彙のみ)。
        assert!(!code.contains("pos_packed"));
        assert!(!code.contains("color_packed"));
    }

    #[test]
    fn pack_ids_full_boundary_exact() {
        assert_eq!(pack_ids(0, 0), 0);
        assert_eq!(pack_ids(65535, 65535), 0xFFFF_FFFF); // 16bit 各最大 = 全ビット
        assert_eq!(unpack_ids(0xFFFF_FFFF), (65535, 65535));
        // 64bit 版は u32 全域で truncation なし (境界往復)。
        let p = pack_ids_64(u32::MAX, u32::MAX);
        assert_eq!(unpack_ids_64(p), (u32::MAX, u32::MAX));
        assert_eq!(unpack_ids_64(pack_ids_64(1, 0)), (1, 0));
    }

    #[test]
    #[should_panic(expected = "pack_ids 契約違反")]
    fn pack_ids_rejects_primitive_aliasing_overflow() {
        // CC-1: primitive >= 65536 は 16bit 欄への静寂エイリアスのため拒否。
        let _ = pack_ids(65536, 0);
    }

    #[test]
    #[should_panic(expected = "pack_ids 契約違反")]
    fn pack_ids_rejects_instance_aliasing_overflow() {
        let _ = pack_ids(0, 70000);
    }

    #[test]
    fn barycentric_exact_rationals_and_degenerate_fallback() {
        // 分母 64 (2 の冪) で全中間値 f32 厳密 (CC-2 手導出):
        let v0 = [0.0, 0.0];
        let v1 = [8.0, 0.0];
        let v2 = [0.0, 8.0];
        assert_eq!(
            compute_barycentrics(2.0, 2.0, v0, v1, v2),
            [0.5, 0.25, 0.25]
        );
        assert_eq!(compute_barycentrics(4.0, 4.0, v0, v1, v2), [0.0, 0.5, 0.5]);
        assert_eq!(compute_barycentrics(0.0, 0.0, v0, v1, v2), [1.0, 0.0, 0.0]);
        // 縮退 (全頂点同一点) → 定義済み重心フォールバック (規約ピン)。
        let d = compute_barycentrics(3.0, 3.0, [3.0, 3.0], [3.0, 3.0], [3.0, 3.0]);
        assert_eq!(d, [0.3333333, 0.3333333, 0.3333333]);
    }

    #[test]
    fn decode_mirrors_layout_and_encode_scales_exact() {
        // CC-3 CPU ミラー: w0 = 1024|2048<<16, w1 = 512|(255|0<<8)<<16,
        // w2 = 32767|32767<<16。除数は encode の厳密ミラー (全値 f32 厳密)。
        let words = [
            1024u32 | (2048 << 16),
            512 | (255 << 16),
            32767 | (32767 << 16),
        ];
        assert_eq!(decode_pos(&words), [1.0, 2.0, 0.5]);
        assert_eq!(decode_uv(&words), [1.0, 1.0]);
        // oct 隅 4 点は全て south pole に厳密着地 (fold の解析的逆、-0.0 は
        // IEEE 等値で 0.0 と一致)。
        for (ox, oy) in [(0u8, 0u8), (255, 0), (0, 255), (255, 255)] {
            assert_eq!(decode_normal_oct(ox, oy), [0.0, 0.0, -1.0]);
        }
        // 中心 (127,127) は量子化丸めを含むため許容誤差 1/127.5 の構造検査
        // (厳密ではないことを明示)。
        let n = decode_normal_oct(127, 127);
        assert!(n[2] > 0.99 && n[0].abs() < 0.008 && n[1].abs() < 0.008);
        assert_eq!(WORDS_PER_VERTEX, 3);
    }

    #[test]
    fn generated_wgsl_is_naga_valid_and_vocabulary_pinned() {
        // CC-3: 生成 WGSL を naga でパース + 全セマンティクス検証
        // (gpu_runtime sweep と同系。GPU 不要の純 CPU 検査)。
        let code = generate_bindless_pulling_wgsl(4096);
        assert!(code.contains("const MAX_INSTANCES: u32 = 4096u;"));
        for vocab in ["/ 1024.0", "/ 32767.0", "127.5", "vertex_words", "bary_tex"] {
            assert!(code.contains(vocab), "missing vocab: {vocab}");
        }
        let module = naga::front::wgsl::parse_str(&code).expect("generated WGSL must parse");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator
            .validate(&module)
            .expect("generated WGSL must pass full naga validation");
    }
}
