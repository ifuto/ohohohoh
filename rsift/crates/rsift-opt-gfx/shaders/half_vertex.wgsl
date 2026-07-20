// Half-precision vertex decode (実 dispatch 版)。
//
// CPU ミラー (完全一致): src/frame_worldgen.rs の f16_decode_bits /
// half_unpack_cpu — 浮動小数演算を一切使わない **ビット配置デコード**:
//   sign = h[15] → out[31]
//   exp  = h[14:10], mant = h[9:0]
//   exp == 0        → ±0 (エンコーダ (half_vertex::f32_to_f16) は subnormal を
//                      flush するため decode 側も 0 で正準)
//   exp == 31       → mant==0 ? ±Inf : NaN (0x7FC00000)
//   それ以外        → (exp + 112) << 23 | mant << 13
//                     (= (1 + mant/1024) * 2^(exp-15) とビット完全一致。
//                      pow/div を使わないので実装定義誤差も存在しない)
//
// iGPU コア: 整数/bitcast のみ (旧版は WGSL に存在しない u16 型と
// 実装定義の pow を使っていた)。頂点バッファでは f16 を u32 に 2 個詰めで
// 受け取り、f32 へ展開して後段 (vertex pull 等) に渡す。

struct HalfParams {
    word_count: u32, // 入力 u32 語数 (1 語 = f16 × 2)
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

@group(0) @binding(0) var<uniform> params: HalfParams;
@group(0) @binding(1) var<storage, read> packed_in: array<u32>;
@group(0) @binding(2) var<storage, read_write> decoded_out: array<f32>;

fn f16_decode_bits(h: u32) -> f32 {
    let sign = (h >> 15u) & 1u;
    let exp = (h >> 10u) & 31u;
    let mant = h & 1023u;
    var bits: u32;
    if (exp == 0u) {
        bits = 0u;
    } else if (exp == 31u) {
        if (mant == 0u) {
            bits = 0x7F800000u; // Inf
        } else {
            bits = 0x7FC00000u; // NaN
        }
    } else {
        bits = ((exp + 112u) << 23u) | (mant << 13u);
    }
    return bitcast<f32>(bits | (sign << 31u));
}

@compute @workgroup_size(64, 1, 1)
fn cs_f16_expand(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.word_count) {
        return;
    }
    let w = packed_in[gid.x];
    decoded_out[gid.x * 2u] = f16_decode_bits(w & 0xFFFFu);
    decoded_out[gid.x * 2u + 1u] = f16_decode_bits(w >> 16u);
}
