
//! bytemuck + zerocopyゼロコピー基盤 - GPU転送コピー0回

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct GpuUploadHeader {
    pub magic: u32,
    pub version: u32,
    pub vertex_count: u32,
    pub index_count: u32,
}

pub fn cast_slice_to_bytes<T: Pod>(slice: &[T]) -> &[u8] {
    bytemuck::cast_slice(slice)
}

pub fn cast_bytes_to_slice<T: Pod>(bytes: &[u8]) -> Option<&[T]> {
    if bytes.len() % std::mem::size_of::<T>() != 0 { return None; }
    Some(bytemuck::cast_slice(bytes))
}

pub fn zero_copy_vertex_upload(vertices: &[crate::chunk_mesh::Quantized12ByteVertex]) -> (&GpuUploadHeader, &[u8]) {
    // ヘッダーを先頭に付けてゼロコピーでGPUへ
    // 実際はUpload Heapに直接書き込む想定
    static HEADER: GpuUploadHeader = GpuUploadHeader { magic: 0x52534946, version: 1, vertex_count: 0, index_count: 0 };
    ( &HEADER, cast_slice_to_bytes(vertices) )
}
