//! # RegionZstd — セーブデータ (リージョンファイル) の圧縮辞書選定 & スループット層
//!
//! 出典: ZFS 界隈の実測（zstd-3 は lz4 と同等のレイテンシで gzip 超の圧縮率）、
//! および Minecraft リージョン形式（4KiB セクタ + チャンクヘッダ）。
//!
//! 本モジュールが提供するのは:
//! * [`RegionCodec`] — リージョン単位でチャンクを読み書きする実装
//!   （ヘッダ 8KB = location table + timestamps、本体は zstd レベル可変）
//! * [`CodecChoice`] — レベル/アルゴリズムの自動選定ロジック
//!   （低スペック CPU では zstd-1、通常 zstd-3、書庫 zstd-19 を推奨）
//!
//! 実 zstd 圧縮には既存 `zstd` crate（workspace 依存あり）を使用。

use std::io::{Read, Write};

/// リージョン = 32x32 チャンク。
pub const REGION_CHUNKS: usize = 1024;
/// location テーブル (4096B) + timestamp (4096B)。
pub const HEADER_BYTES: usize = 8192;
pub const SECTOR: usize = 4096;

/// 圧縮ポリシー。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecChoice {
    /// Raw（デバッグ・超高性能 CPU 向け）
    Stored,
    /// 低スペック: zstd レベル 1（最速・十分高圧縮）
    ZstdFast,
    /// デフォルト推奨: zstd レベル 3（ZFS 実測と同等の推奨帯）
    ZstdBalanced,
    /// 書庫: zstd レベル 19（長期保存・アップロード帯）
    ZstdArchive,
}

impl CodecChoice {
    /// CPU コア数と利用可否に応じた自動選定。
    pub fn auto(logical_cores: usize, battery_saver: bool) -> Self {
        if battery_saver || logical_cores <= 2 {
            CodecChoice::ZstdFast
        } else {
            CodecChoice::ZstdBalanced
        }
    }

    fn zstd_level(&self) -> Option<i32> {
        match self {
            CodecChoice::Stored => None,
            CodecChoice::ZstdFast => Some(1),
            CodecChoice::ZstdBalanced => Some(3),
            CodecChoice::ZstdArchive => Some(19),
        }
    }
}

/// リージョンの location テーブル 1 エントリ。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Location {
    pub offset_sectors: u32,
    pub sectors: u32,
}

/// リージョン・コーデック（インメモリで組み立ててから一括 write する形式）。
pub struct RegionCodec {
    pub choice: CodecChoice,
    locations: [Location; REGION_CHUNKS],
    chunks: [Option<Vec<u8>>; REGION_CHUNKS],
}

impl RegionCodec {
    pub fn new(choice: CodecChoice) -> Self {
        const NONE: Option<Vec<u8>> = None;
        Self {
            choice,
            locations: [Location::default(); REGION_CHUNKS],
            chunks: [NONE; REGION_CHUNKS],
        }
    }

    /// チャンク (cx, cz 0..32) の生データを登録。
    pub fn put_chunk(&mut self, cx: usize, cz: usize, raw: &[u8]) {
        debug_assert!(cx < 32 && cz < 32);
        let idx = cz * 32 + cx;
        let body = match self.choice.zstd_level() {
            None => raw.to_vec(),
            Some(level) => zstd::stream::encode_all(std::io::Cursor::new(raw), level)
                .expect("zstd encode"),
        };
        self.chunks[idx] = Some(body);
    }

    /// チャンクの読み出し（展開済み）。
    pub fn get_chunk(&self, cx: usize, cz: usize) -> Option<Vec<u8>> {
        debug_assert!(cx < 32 && cz < 32);
        let idx = cz * 32 + cx;
        self.chunks[idx].as_ref().map(|body| {
            if self.choice.zstd_level().is_none() {
                body.clone()
            } else {
                zstd::stream::decode_all(std::io::Cursor::new(body)).expect("zstd decode")
            }
        })
    }

    /// 完全なリージョンファイルを構築（sector alignment 込み）。
    pub fn build_file(&mut self) -> Vec<u8> {
        // まず本体を並べて location を決定
        let mut body: Vec<u8> = Vec::new();
        let mut offset = HEADER_BYTES / SECTOR; // セクタ単位
        for i in 0..REGION_CHUNKS {
            if let Some(c) = &self.chunks[i] {
                // 4 バイト長 + 1 バイト形式を付す（vanilla 5B ヘッダ互換）
                let mut rec = Vec::with_capacity(c.len() + 5);
                let len_field = (c.len() + 1) as u32;
                rec.write_all(&len_field.to_be_bytes()).unwrap();
                let fmt: u8 = match self.choice {
                    CodecChoice::Stored => 1, // 1=gzip 扱いではないが raw 代替
                    _ => 2,                   // 2=zlib 系/自前
                };
                rec.push(fmt);
                rec.extend_from_slice(c);
                // セクタパディング
                let rem = SECTOR - (rec.len() % SECTOR);
                if rem < SECTOR {
                    rec.resize(rec.len() + rem, 0);
                }
                self.locations[i] = Location {
                    offset_sectors: offset as u32,
                    sectors: (rec.len() / SECTOR) as u32,
                };
                offset += rec.len() / SECTOR;
                body.extend_from_slice(&rec);
            } else {
                self.locations[i] = Location::default();
            }
        }
        let mut out = Vec::with_capacity(HEADER_BYTES + body.len());
        for loc in &self.locations {
            let v: u32 = (loc.offset_sectors << 8) | (loc.sectors & 0xFF);
            out.write_all(&v.to_be_bytes()).unwrap();
        }
        // timestamps (all zero は実機では epoch)
        out.resize(HEADER_BYTES, 0);
        out.extend_from_slice(&body);
        out
    }

    /// 構築済みファイルのチャンク数・推定圧縮率。
    pub fn stats(&self) -> (usize, f64) {
        let mut n = 0usize;
        let mut raw: usize = 0;
        let mut packed: usize = 0;
        for c in self.chunks.iter().flatten() {
            n += 1;
            packed += c.len();
            if let Ok(d) = zstd::stream::decode_all(std::io::Cursor::new(c)) {
                raw += d.len();
            }
        }
        let ratio = if raw > 0 { packed as f64 / raw as f64 } else { 1.0 };
        (n, ratio)
    }
}

/// リージョンファイルの走査（読み出し側検証用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionScan {
    pub file_bytes: usize,
    pub used_sectors: u32,
    pub chunks: u32,
}

pub fn scan_file(bytes: &[u8]) -> std::io::Result<RegionScan> {
    if bytes.len() < HEADER_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "region file too small",
        ));
    }
    let mut used = (HEADER_BYTES / SECTOR) as u32;
    let mut chunks = 0u32;
    for i in 0..REGION_CHUNKS {
        let mut b = [0u8; 4];
        let mut rdr = &bytes[i * 4..];
        rdr.read_exact(&mut b)?;
        let v = u32::from_be_bytes(b);
        let sectors = v & 0xFF;
        if sectors > 0 {
            used += sectors;
            chunks += 1;
        }
    }
    Ok(RegionScan {
        file_bytes: bytes.len(),
        used_sectors: used,
        chunks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_chunk(seed: u8) -> Vec<u8> {
        // 実チャンクに似せた高圧縮率データ（stone 繰り返し + ノイズ）
        let mut v = vec![seed; 64 * 1024];
        for i in 0..1024 {
            v[i] = (i as u16 % 251) as u8;
        }
        v
    }

    #[test]
    fn roundtrip_chunk() {
        let mut rc = RegionCodec::new(CodecChoice::ZstdBalanced);
        let data = sample_chunk(7);
        rc.put_chunk(3, 5, &data);
        let back = rc.get_chunk(3, 5).unwrap();
        assert_eq!(back, data);
        assert!(rc.get_chunk(0, 0).is_none());
    }

    #[test]
    fn file_layout_is_sector_aligned() {
        let mut rc = RegionCodec::new(CodecChoice::ZstdFast);
        rc.put_chunk(0, 0, &sample_chunk(1));
        rc.put_chunk(1, 0, &sample_chunk(2));
        let file = rc.build_file();
        assert!(file.len() % SECTOR == 0);
        let scan = scan_file(&file).unwrap();
        assert_eq!(scan.chunks, 2);
        assert!(scan.file_bytes >= HEADER_BYTES + 2 * SECTOR);
    }

    #[test]
    fn zstd_beats_raw_on_chunk_data() {
        let mut rc = RegionCodec::new(CodecChoice::ZstdBalanced);
        rc.put_chunk(0, 0, &sample_chunk(0));
        let (n, ratio) = rc.stats();
        assert_eq!(n, 1);
        assert!(ratio < 0.2, "zstd should smash repetitive chunk data: {ratio}");
    }

    #[test]
    fn auto_policy_low_spec() {
        assert_eq!(CodecChoice::auto(2, false), CodecChoice::ZstdFast);
        assert_eq!(CodecChoice::auto(8, false), CodecChoice::ZstdBalanced);
        assert_eq!(CodecChoice::auto(8, true), CodecChoice::ZstdFast);
    }
}
