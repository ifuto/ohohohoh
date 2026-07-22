// Virtual (sparse) texture address translation. (page, mip) の要求を間接
// ページテーブル経由で物理スロットへ解決し、out[idx] に書き出す実カーネル
// (2026-07-23 wave 42: 出力を破棄していた reference 版から根治)。
//
// CPU ミラー (完全一致): src/sparse_texture.rs translate_with_fallback。
//   直接照会 → 未常駐なら最細 resident 祖先 mip (mip-1 → 0 の順の最初)
//   へ退化 → 全段未常駐または範囲外なら INVALID。
// フォールバックはテクスチャ fetch 劣化の教科書的規則:
// 「要求 mip が無ければ、得られる中で最も詳細な祖先 mip を使う」。
//
// 上傳契約 (CPU 側責務): pageTable と out は
// pageCount * (maxMip + 1) 要素以上 (u32 乗算の範囲内に収める)。

const INVALID : u32 = 0xFFFFFFFFu;

struct U {
    maxMip : u32,    // 有効 mip は 0..=maxMip
    pageCount : u32, // pageTable/out の論理ページ数
};

@group(0) @binding(0) var<uniform> u : U;
@group(0) @binding(1) var<storage, read> pageTable : array<u32>; // idx -> slot or INVALID
@group(0) @binding(2) var<storage, read_write> outPages : array<u32>; // idx -> 解決済み slot

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid : vec3<u32>) {
    let page = gid.x;
    let mip = gid.y;
    if (page >= u.pageCount || mip > u.maxMip) {
        return; // 契約外レーン: 対応する out 要素自体が存在しない
    }
    let stride = u.maxMip + 1u;
    let base = page * stride;
    var phys = pageTable[base + mip];
    if (phys == INVALID) {
        // 最細 resident 祖先 mip を mip-1 -> 0 の順に探索
        // (WGSL は降順 for を直接書けないのでオフセット k で昇順に回す)。
        for (var k = 1u; k <= mip; k = k + 1u) {
            let cand = pageTable[base + mip - k];
            if (cand != INVALID) {
                phys = cand;
                break;
            }
        }
    }
    outPages[base + mip] = phys;
}
