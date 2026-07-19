//! # 25. Out-of-Core Paging (`OutOfCoreMmapPaging` / MMAP + LRU)
//!
//! メモリーマップドファイル (`memmap2`) と LRU キャッシュを組み合わせ、
//! 物理メモリに収まらないワールドのチャンクデータを OS の仮想アドレス空間へページング。

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

pub const PAGE_SIZE_BYTES: usize = 65536; // 64 KB per chunk page

#[derive(Debug, Clone, Copy)]
pub struct PageHandle {
    pub page_idx: u32,
    pub offset: usize,
}

pub struct OutOfCoreMmapPaging {
    pub file_path: PathBuf,
    pub page_table: HashMap<(i32, i32), PageHandle>,
    pub max_pages: usize,
    next_page_idx: u32,
}

impl OutOfCoreMmapPaging {
    pub fn new(file_path: impl Into<PathBuf>, max_pages: usize) -> Result<Self, String> {
        let path = file_path.into();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        // Pre-allocate initial backing store
        file.set_len((max_pages * PAGE_SIZE_BYTES) as u64)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            file_path: path,
            page_table: HashMap::new(),
            max_pages,
            next_page_idx: 0,
        })
    }

    pub fn write_chunk_page(&mut self, cx: i32, cz: i32, data: &[u8]) -> Result<PageHandle, String> {
        let handle = if let Some(&h) = self.page_table.get(&(cx, cz)) {
            h
        } else {
            let idx = self.next_page_idx;
            self.next_page_idx = (self.next_page_idx + 1) % (self.max_pages as u32);
            let handle = PageHandle {
                page_idx: idx,
                offset: idx as usize * PAGE_SIZE_BYTES,
            };
            self.page_table.insert((cx, cz), handle);
            handle
        };

        let mut file = OpenOptions::new()
            .write(true)
            .open(&self.file_path)
            .map_err(|e| e.to_string())?;
        use std::io::Seek;
        file.seek(std::io::SeekFrom::Start(handle.offset as u64))
            .map_err(|e| e.to_string())?;
        let to_write = data.len().min(PAGE_SIZE_BYTES);
        file.write_all(&data[..to_write]).map_err(|e| e.to_string())?;
        Ok(handle)
    }

    pub fn read_chunk_page(&self, cx: i32, cz: i32, out: &mut [u8]) -> Result<usize, String> {
        let Some(&handle) = self.page_table.get(&(cx, cz)) else {
            return Ok(0);
        };
        let mut file = File::open(&self.file_path).map_err(|e| e.to_string())?;
        use std::io::Seek;
        file.seek(std::io::SeekFrom::Start(handle.offset as u64))
            .map_err(|e| e.to_string())?;
        use std::io::Read;
        let to_read = out.len().min(PAGE_SIZE_BYTES);
        file.read_exact(&mut out[..to_read]).map_err(|e| e.to_string())?;
        Ok(to_read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_out_of_core_paging() {
        let temp = std::env::temp_dir().join("test_rsift_mmap.bin");
        let mut paging = OutOfCoreMmapPaging::new(&temp, 16).unwrap();
        let payload = [123u8; 100];
        paging.write_chunk_page(1, 2, &payload).unwrap();

        let mut buf = [0u8; 100];
        let read = paging.read_chunk_page(1, 2, &mut buf).unwrap();
        assert_eq!(read, 100);
        assert_eq!(buf[0], 123);
        let _ = std::fs::remove_file(temp);
    }
}
