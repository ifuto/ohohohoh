//! Phase 1/3 — Anvil-style region I/O: LZ4 hot path, Zstd cold, CRC, generations, mmap index.

use lz4_flex::{compress_prepend_size, decompress_size_prepended};
use memmap2::Mmap;
use parking_lot::RwLock;
use rustc_hash::FxHashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tracing::debug;

pub const SECTOR: usize = 4096;
pub const HEADER_BYTES: usize = 8192; // 1024*4 locations + 1024*4 timestamps

#[derive(Debug, Error)]
pub enum RegionError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("crc mismatch")]
    Crc,
    #[error("bad chunk")]
    BadChunk,
    #[error("decompress")]
    Decompress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionKind {
    Lz4 = 1,
    Zstd = 2,
}

#[derive(Debug, Clone)]
pub struct ChunkBlob {
    pub data: Vec<u8>,
    pub generation: u32,
    pub compression: CompressionKind,
}

fn crc32(data: &[u8]) -> u32 {
    // Small portable CRC32 (IEEE)
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (!(crc & 1)).wrapping_add(1);
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[derive(Debug)]
pub struct RegionIndexCache {
    /// (region_x, region_z) → offsets table (1024 u32)
    map: RwLock<FxHashMap<(i32, i32), [u32; 1024]>>,
}

impl Default for RegionIndexCache {
    fn default() -> Self {
        Self::new()
    }
}

impl RegionIndexCache {
    pub fn new() -> Self {
        Self {
            map: RwLock::new(FxHashMap::default()),
        }
    }

    pub fn get(&self, rx: i32, rz: i32) -> Option<[u32; 1024]> {
        self.map.read().get(&(rx, rz)).copied()
    }

    pub fn insert(&self, rx: i32, rz: i32, locs: [u32; 1024]) {
        self.map.write().insert((rx, rz), locs);
    }

    pub fn invalidate(&self, rx: i32, rz: i32) {
        self.map.write().remove(&(rx, rz));
    }
}

pub struct RegionFile {
    path: PathBuf,
    file: File,
    locations: [u32; 1024],
    generations: [u32; 1024],
}

impl RegionFile {
    pub fn open(dir: &Path, region_x: i32, region_z: i32) -> Result<Self, RegionError> {
        fs::create_dir_all(dir)?;
        let path = dir.join(format!("r.{}.{}.mca", region_x, region_z));
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&path)?;
        let meta = file.metadata()?;
        let mut locations = [0u32; 1024];
        let mut generations = [0u32; 1024];
        if meta.len() < HEADER_BYTES as u64 {
            let zeros = vec![0u8; HEADER_BYTES];
            file.write_all(&zeros)?;
            file.set_len(HEADER_BYTES as u64)?;
        } else {
            let mut hdr = [0u8; HEADER_BYTES];
            file.seek(SeekFrom::Start(0))?;
            file.read_exact(&mut hdr)?;
            for i in 0..1024 {
                let o = i * 4;
                locations[i] = u32::from_be_bytes([hdr[o], hdr[o + 1], hdr[o + 2], hdr[o + 3]]);
            }
            // Reuse timestamp slots as generation counter (custom Rsift extension).
            for i in 0..1024 {
                let o = 4096 + i * 4;
                generations[i] =
                    u32::from_be_bytes([hdr[o], hdr[o + 1], hdr[o + 2], hdr[o + 3]]);
            }
        }
        Ok(Self {
            path,
            file,
            locations,
            generations,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn local_index(cx: i32, cz: i32) -> usize {
        let lx = cx.rem_euclid(32) as usize;
        let lz = cz.rem_euclid(32) as usize;
        lx + lz * 32
    }

    pub fn read_chunk(&mut self, cx: i32, cz: i32) -> Result<Option<ChunkBlob>, RegionError> {
        let idx = Self::local_index(cx, cz);
        let loc = self.locations[idx];
        if loc == 0 {
            return Ok(None);
        }
        let sector_off = ((loc >> 8) as u64) * SECTOR as u64;
        let sector_count = (loc & 0xff) as u64;
        if sector_count == 0 {
            return Ok(None);
        }
        let mut buf = vec![0u8; (sector_count as usize) * SECTOR];
        self.file.seek(SeekFrom::Start(sector_off))?;
        self.file.read_exact(&mut buf)?;
        if buf.len() < 9 {
            return Err(RegionError::BadChunk);
        }
        let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        if len + 4 > buf.len() {
            return Err(RegionError::BadChunk);
        }
        let comp = match buf[4] {
            1 => CompressionKind::Lz4,
            2 => CompressionKind::Zstd,
            _ => return Err(RegionError::BadChunk),
        };
        let payload = &buf[5..4 + len];
        if payload.len() < 4 {
            return Err(RegionError::BadChunk);
        }
        let (body, crc_bytes) = payload.split_at(payload.len() - 4);
        let expect = u32::from_le_bytes(crc_bytes.try_into().unwrap());
        if crc32(body) != expect {
            return Err(RegionError::Crc);
        }
        let data = match comp {
            CompressionKind::Lz4 => {
                decompress_size_prepended(body).map_err(|_| RegionError::Decompress)?
            }
            CompressionKind::Zstd => {
                zstd::decode_all(body).map_err(|_| RegionError::Decompress)?
            }
        };
        Ok(Some(ChunkBlob {
            data,
            generation: self.generations[idx],
            compression: comp,
        }))
    }

    pub fn write_chunk(
        &mut self,
        cx: i32,
        cz: i32,
        raw: &[u8],
        prefer_zstd: bool,
    ) -> Result<(), RegionError> {
        let idx = Self::local_index(cx, cz);
        let (comp, compressed) = if prefer_zstd || raw.len() > 64 * 1024 {
            (
                CompressionKind::Zstd,
                zstd::encode_all(raw, 3).map_err(|e| RegionError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?,
            )
        } else {
            (CompressionKind::Lz4, compress_prepend_size(raw))
        };
        let mut body = compressed;
        let c = crc32(&body);
        body.extend_from_slice(&c.to_le_bytes());
        let len = (body.len() + 1) as u32; // + compression byte
        let mut record = Vec::with_capacity(4 + 1 + body.len());
        record.extend_from_slice(&len.to_be_bytes());
        record.push(comp as u8);
        record.extend_from_slice(&body);
        while record.len() % SECTOR != 0 {
            record.push(0);
        }
        let sectors = (record.len() / SECTOR) as u32;
        let file_len = self.file.metadata()?.len().max(HEADER_BYTES as u64);
        let mut start_sector = (file_len / SECTOR as u64) as u32;
        if start_sector < 2 {
            start_sector = 2;
        }
        self.file
            .seek(SeekFrom::Start(start_sector as u64 * SECTOR as u64))?;
        self.file.write_all(&record)?;
        self.locations[idx] = (start_sector << 8) | (sectors & 0xff);
        self.generations[idx] = self.generations[idx].wrapping_add(1);
        self.flush_header()?;
        debug!(cx, cz, sectors, gen = self.generations[idx], "region write");
        Ok(())
    }

    fn flush_header(&mut self) -> Result<(), RegionError> {
        let mut hdr = [0u8; HEADER_BYTES];
        for i in 0..1024 {
            let b = self.locations[i].to_be_bytes();
            hdr[i * 4..i * 4 + 4].copy_from_slice(&b);
            let g = self.generations[i].to_be_bytes();
            hdr[4096 + i * 4..4096 + i * 4 + 4].copy_from_slice(&g);
        }
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&hdr)?;
        self.file.flush()?;
        Ok(())
    }

    /// Rollback chunk to previous generation marker (clears location — soft delete).
    pub fn rollback_slot(&mut self, cx: i32, cz: i32) -> Result<(), RegionError> {
        let idx = Self::local_index(cx, cz);
        self.locations[idx] = 0;
        self.generations[idx] = self.generations[idx].wrapping_add(1);
        self.flush_header()
    }
}

/// mmap auxiliary index file (offsets only) for fast existence checks.
pub struct MmapRegionIndex {
    _file: File,
    mmap: Mmap,
    region_x: i32,
    region_z: i32,
}

impl MmapRegionIndex {
    pub fn open(path: &Path, region_x: i32, region_z: i32) -> Result<Self, RegionError> {
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        Ok(Self {
            _file: file,
            mmap,
            region_x,
            region_z,
        })
    }

    pub fn has_chunk(&self, cx: i32, cz: i32) -> bool {
        if self.mmap.len() < 4096 {
            return false;
        }
        let lx = cx.rem_euclid(32) as usize;
        let lz = cz.rem_euclid(32) as usize;
        let i = (lx + lz * 32) * 4;
        let loc = u32::from_be_bytes([
            self.mmap[i],
            self.mmap[i + 1],
            self.mmap[i + 2],
            self.mmap[i + 3],
        ]);
        loc != 0
    }

    pub fn region(&self) -> (i32, i32) {
        (self.region_x, self.region_z)
    }
}

pub fn now_secs() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_lz4() {
        let dir = std::env::temp_dir().join("rsift_region_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut r = RegionFile::open(&dir, 0, 0).unwrap();
        let payload = b"hello-chunk-payload-0123456789".repeat(100);
        r.write_chunk(0, 0, &payload, false).unwrap();
        let got = r.read_chunk(0, 0).unwrap().unwrap();
        assert_eq!(got.data, payload);
        assert_eq!(got.compression, CompressionKind::Lz4);
        let _ = fs::remove_dir_all(&dir);
    }
}
