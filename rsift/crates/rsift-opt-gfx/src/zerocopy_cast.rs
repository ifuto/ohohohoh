//! bytemuck + zerocopyゼロコピー基盤 - GPU転送コピー0回
//!
//! 設計ノート (2026-07-23 wave 51 監査):
//! 実消費される API は `cast_slice_to_bytes` / `cast_bytes_to_slice` の
//! 2 関数のみ (render_pipeline.rs:419/:712/:787/:793)。かつて存在した
//! `GpuUploadHeader`/`zero_copy_vertex_upload` は workspace 全域で消費者
//! ゼロの上、static ヘッダが `vertex_count: 0` 固定でペイロード長と矛盾
//! する (読み手に嘘の枚数を提示する) ため撤去した — ヘッダ前置の
//! ワイヤ形式が実際に必要になった時点で、実カウントを保持する所有型として
//! 再設計するのが正しい (願望スタブを残さない)。

use bytemuck::Pod;

pub fn cast_slice_to_bytes<T: Pod>(slice: &[T]) -> &[u8] {
    bytemuck::cast_slice(slice)
}

/// バイト列を Pod スライスへ再解釈する。失敗時はパニックではなく `None`。
///
/// **契約 (wave 51 厳格化)**: `T` は ZST 禁止。bytemuck 自身は ZST 宛を
/// 空入力限定で受理する (internal.rs: ZST なら出力も空) が、この API が
/// 扱うのは GPU 転送バッファの再解釈であり ZST の正当用途は存在しない。
/// ZST を許すと `bytes.len() % size_of::<T>()` の剰余が**ゼロ除算パニック**
/// になる (意味不明なメッセージで実装事故と区別できない) ため、入口で
/// 明示的に契約アサートする。
pub fn cast_bytes_to_slice<T: Pod>(bytes: &[u8]) -> Option<&[T]> {
    assert!(
        std::mem::size_of::<T>() > 0,
        "cast_bytes_to_slice 契約違反: ZST 宛のバイト再解釈は無意味 (GPU 転送単位は非零サイズ型のみ)"
    );
    if bytes.len() % std::mem::size_of::<T>() != 0 {
        return None;
    }
    // アライメント検査: bytemuck::cast_slice は入力先頭番地が T の align を
    // 満たさない場合に**パニック**する (TargetAlignmentGreaterAndInputNotAligned)。
    // 特に空の `Vec<u8>` は dangling ポインタ (align 1) を返すため、
    // 「長さ 0 は常に合法」と信じた呼出側で致命的パニックとなる
    // (監査 2026-07-22 M-4: render_pipeline::frame() の quad_budget 経路で
    //  CI 診断 B1-B22 により実観測された latent panic)。Option 拒否に統一する。
    if (bytes.as_ptr() as usize) % std::mem::align_of::<T>() != 0 {
        return None;
    }
    Some(bytemuck::cast_slice(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk_mesh::Quantized12ByteVertex;
    use bytemuck::Zeroable;

    /// テスト用 ZST (Pod の unsafe 契約: Copy + パディング無しは ZST で自明に成立)。
    #[derive(Clone, Copy)]
    struct ZeroSized;
    unsafe impl Zeroable for ZeroSized {}
    unsafe impl Pod for ZeroSized {}

    #[test]
    fn cast_roundtrip_preserves_bytes() {
        let verts: Vec<Quantized12ByteVertex> = (0..3)
            .map(|i| Quantized12ByteVertex::encode(i as f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0))
            .collect();
        let bytes = cast_slice_to_bytes(&verts);
        assert_eq!(bytes.len(), 3 * 12);
        let back: &[Quantized12ByteVertex] = cast_bytes_to_slice(bytes).unwrap();
        assert_eq!(back.len(), 3);
        assert_eq!(cast_slice_to_bytes(back), bytes);
    }

    #[test]
    fn cast_bytes_to_slice_rejects_ragged_buffer() {
        let raw = [0u8; 13]; // 頂点 12B の倍数でない → None
        assert!(cast_bytes_to_slice::<Quantized12ByteVertex>(&raw).is_none());
        // 空バッファは Some(空) (0 % 12 == 0)。bytemuck は bytemuck::cast_slice
        // で目的型 align (4B) のポインタ検査を走らせるため、スタックの
        // [u8; 0] (align 1、番地は実行依存) ではなく align(4) 保証の受け皿で
        // 固定する (テストをポインタ配置の偶然に寄せない)。
        #[repr(align(4))]
        struct Aligned4([u8; 0]);
        let empty = Aligned4([]);
        let ok: &[Quantized12ByteVertex] = cast_bytes_to_slice(&empty.0).unwrap();
        assert!(ok.is_empty());
    }

    /// wave 51-1: ZST 宛のキャストは契約違反 (剰余ゼロ除算パニックの明示化)。
    #[test]
    #[should_panic(expected = "ZST 宛のバイト再解釈は無意味")]
    fn cast_bytes_to_slice_rejects_zero_sized_target() {
        let empty: [u8; 0] = [];
        let _ = cast_bytes_to_slice::<ZeroSized>(&empty);
    }

    /// wave 51-1 補遺: ZST でも非空入力で同じ契約パニック (旧実装は空でも
    /// `% 0` で不定メッセージのパニックになっていた)。
    #[test]
    #[should_panic(expected = "ZST 宛のバイト再解釈は無意味")]
    fn cast_bytes_to_slice_rejects_zero_sized_target_nonempty() {
        let buf = [0u8; 4];
        let _ = cast_bytes_to_slice::<ZeroSized>(&buf);
    }

    /// wave 51-2: ミスアライメントを配置の偶然に頼らず構成する —
    /// align(16) 保証バッファの +1 オフセットは align(4) 型に対し決定的に
    /// 非整列 (1 % 4 = 1) なので None、+4 オフセットは整列のまま Some。
    #[test]
    fn cast_bytes_to_slice_alignment_boundary_exact() {
        #[repr(align(16))]
        struct Aligned16([u8; 32]);
        let buf = Aligned16([0u8; 32]);
        let base = buf.0.as_ptr() as usize;
        assert_eq!(base % 16, 0, "前提: align(16) 保証");
        // +1: 非整列 → サイズが合っていても None
        assert!(cast_bytes_to_slice::<Quantized12ByteVertex>(&buf.0[1..13]).is_none());
        // +4: 整列かつ 12B ちょうど → Some(1 頂点)
        let ok: &[Quantized12ByteVertex] = cast_bytes_to_slice(&buf.0[4..16]).unwrap();
        assert_eq!(ok.len(), 1);
    }

    #[test]
    fn cast_bytes_to_slice_empty_vec_is_none_not_panic() {
        // M-4 実回帰: 空の Vec<u8> (dangling ptr, align 1) を渡すと旧実装は
        // bytemuck::cast_slice のアライメント検査でパニックした。
        // render_pipeline::frame() の quad_budget 経路は pull バイト 0 の
        // フレームでこれに到達し得る (デモ/空地形の通常実行でも)。
        let empty: Vec<u8> = Vec::new();
        assert!(cast_bytes_to_slice::<Quantized12ByteVertex>(&empty).is_none());
        assert!(cast_bytes_to_slice::<crate::packed4::PackedPullQuad>(&empty).is_none());
        // ラギッドは引き続き None、実バッファは従来どおり Some。
        let ragged = vec![0u8; 10];
        assert!(cast_bytes_to_slice::<crate::packed4::PackedPullQuad>(&ragged).is_none());
        let real = vec![0u8; 24];
        assert!(cast_bytes_to_slice::<Quantized12ByteVertex>(&real).is_some());
    }
}
