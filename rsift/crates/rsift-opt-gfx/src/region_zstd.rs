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
    /// 範囲外座標は `false` を返して拒否する (旧実装は debug_assert /
    /// リリースビルドでは配列境界パニックだった — 監査 2026-07-22 L-3)。
    pub fn put_chunk(&mut self, cx: usize, cz: usize, raw: &[u8]) -> bool {
        if cx >= 32 || cz >= 32 {
            return false;
        }
        let idx = cz * 32 + cx;
        let body = match self.choice.zstd_level() {
            None => raw.to_vec(),
            Some(level) => zstd::stream::encode_all(std::io::Cursor::new(raw), level)
                .expect("zstd encode"),
        };
        self.chunks[idx] = Some(body);
        true
    }

    /// チャンクの読み出し（展開済み）。
    /// 範囲外座標は `None` (L-3: 旧実装は debug_assert / リリースでパニック)。
    ///
    /// 展開は無制限の `decode_all` を使うが、対象は本モジュールがメモリ上で
    /// `encode_all` した自己生成バイト列に限定される (chunks は private) —
    /// wave 98 CW-1 で根治した disk 経路 (外部由来 .rmesh) の展開ボムとは
    /// 危険度が異なる (wave 99 CX-3: CW-5 起票の再評価として doc 明文化)。
    /// メモリ破壊が無い限り `expect` は不到達で fail-loud のまま。
    pub fn get_chunk(&self, cx: usize, cz: usize) -> Option<Vec<u8>> {
        if cx >= 32 || cz >= 32 {
            return None;
        }
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
    ///
    /// # Panics (wave 99 CX-1)
    /// 1 チャンクの記録が 255 セクタ (=1,044,480 B、vanilla location の
    /// 8-bit セクタ数上限) を超える場合は panic する。旧実装は `sectors & 0xFF`
    /// で静寂ラップして reader からは読めない破損ファイルを返していた
    /// (Store 選択時の >1MiB 生チャンクで到達可能)。データ駆動の失敗に
    /// `Result` が必要な呼び出し側は [`Self::build_file_checked`] を使う。
    /// zstd 選択の実チャンク (<64 KiB 級) では非到達 (コメントの実測根拠)。
    pub fn build_file(&mut self) -> Vec<u8> {
        self.build_file_checked()
            .expect("chunk record must fit within vanilla 255-sector location limit")
    }

    /// [`Self::build_file`] の fail-loud 版。255-sector 超過で `Err` を返す
    /// (その場合ファイルは構築されない: 旧実装は `& 0xFF` で静寂破損させた)。
    pub fn build_file_checked(&mut self) -> Result<Vec<u8>, String> {
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
                let sectors = rec.len() / SECTOR;
                // wave 99 CX-1: vanilla location 8-bit セクタ数の上限。
                // 境界 (Python 機械検算): body = 255*4096-5 = 1,044,475 で
                //   sectors=255 受理、+1 B で 256 → Err/panic。
                if sectors > 0xFF {
                    return Err(format!(
                        "chunk {i} uses {sectors} sectors > 255-sector vanilla limit"
                    ));
                }
                self.locations[i] = Location {
                    offset_sectors: offset as u32,
                    sectors: sectors as u32,
                };
                offset += sectors;
                body.extend_from_slice(&rec);
            } else {
                self.locations[i] = Location::default();
            }
        }
        let mut out = Vec::with_capacity(HEADER_BYTES + body.len());
        for loc in &self.locations {
            // vanilla 形式: offset 24bit + セクタ数 8bit。
            // CX-1 ガードにより sectors ≤ 255 が不変条件化 → `& 0xFF` は no-op
            // (構造 → wire の一貫性のため保持)。
            let v: u32 = (loc.offset_sectors << 8) | (loc.sectors & 0xFF);
            out.write_all(&v.to_be_bytes()).unwrap();
        }
        // timestamps (all zero は実機では epoch)
        out.resize(HEADER_BYTES, 0);
        out.extend_from_slice(&body);
        Ok(out)
    }

    /// 構築済みファイルのチャンク数・推定圧縮率。
    /// 【L-4】Stored (無圧縮) では body 長をそのまま raw/packed 両辺に計上する
    /// (旧実装は zstd デコード失敗で raw 未計上のまま比を構築していた)。
    /// decode 失敗時の raw 未計上も自己生成データ限定では不到達 (CX-3 同根拠)。
    pub fn stats(&self) -> (usize, f64) {
        let mut n = 0usize;
        let mut raw: usize = 0;
        let mut packed: usize = 0;
        for c in self.chunks.iter().flatten() {
            n += 1;
            packed += c.len();
            if self.choice.zstd_level().is_none() {
                raw += c.len();
            } else if let Ok(d) = zstd::stream::decode_all(std::io::Cursor::new(c)) {
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
        let off_sectors = v >> 8;
        let sectors = v & 0xFF;
        if sectors > 0 {
            // wave 99 CX-2: 破損/細工ファイルへの fail-loud 検査。
            // 旧実装は location スパンを無検証で受理し、ヘッダ内を指す
            // offset や file 境界を超えるスパンを「正常」として報告し得た。
            if off_sectors < (HEADER_BYTES / SECTOR) as u32 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("chunk {i} offset {off_sectors} points into region header"),
                ));
            }
            let end = (off_sectors + sectors) as usize * SECTOR;
            if end > bytes.len() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "chunk {i} span end {end} exceeds file length {}",
                        bytes.len()
                    ),
                ));
            }
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

    #[test]
    fn out_of_range_access_is_rejected_without_panic() {
        // L-3 回帰: 旧実装は debug_assert/リリースパニック。
        let mut rc = RegionCodec::new(CodecChoice::ZstdFast);
        let data = sample_chunk(3);
        assert!(!rc.put_chunk(32, 0, &data));
        assert!(!rc.put_chunk(0, 32, &data));
        assert!(!rc.put_chunk(32, 32, &data));
        assert!(!rc.put_chunk(usize::MAX, usize::MAX, &data));
        assert!(rc.get_chunk(32, 0).is_none());
        assert!(rc.get_chunk(0, 32).is_none());
        assert!(rc.get_chunk(usize::MAX, 1).is_none());
        // 範囲内は従来どおり受理・厳密往復。
        assert!(rc.put_chunk(31, 31, &data));
        assert_eq!(rc.get_chunk(31, 31).unwrap(), data);
        let (n, _) = rc.stats();
        assert_eq!(n, 1, "拒否分は格納されない");
    }

    #[test]
    fn stored_stats_ratio_counts_both_sides() {
        // L-4 回帰: Stored で raw 未計上だと ratio が虚偽に。
        let mut rc = RegionCodec::new(CodecChoice::Stored);
        let d1 = sample_chunk(11);
        let d2 = sample_chunk(12);
        rc.put_chunk(0, 0, &d1);
        rc.put_chunk(1, 0, &d2);
        let (n, ratio) = rc.stats();
        assert_eq!(n, 2);
        assert_eq!(
            ratio.to_bits(),
            1.0f64.to_bits(),
            "無圧縮は packed/raw が厳密 1.0"
        );
    }

    /// wave 99 CX-1: 255 セクタ丁度 (body = 255*4096-5 = 1,044,475 B) は受理、
    /// location テーブルのセクタ数も厳密 255 (旧 `& 0xFF` ラップの境界ピン)。
    #[test]
    fn sector_255_boundary_exact_ok() {
        let mut rc = RegionCodec::new(CodecChoice::Stored);
        let body = vec![0xABu8; 255 * SECTOR - 5];
        assert!(rc.put_chunk(0, 0, &body));
        let file = rc.build_file();
        let entry = u32::from_be_bytes(file[0..4].try_into().unwrap());
        assert_eq!(entry >> 8, 2, "先頭オフセットはヘッダ直後の 2 セクタ");
        assert_eq!(entry & 0xFF, 255, "セクタ数は厳密 255");
        let scan = scan_file(&file).unwrap();
        assert_eq!(scan.chunks, 1);
        assert_eq!(scan.used_sectors, 2 + 255);
    }

    /// wave 99 CX-1: +1 B で 256 セクタ → checked は Err、build_file は panic
    /// (旧実装は `& 0xFF` で 0 に潰して location=未登録相当の破損ファイルを返した)。
    #[test]
    fn sector_256_is_err_and_build_file_panics() {
        let mut rc = RegionCodec::new(CodecChoice::Stored);
        let body = vec![0xCDu8; 255 * SECTOR - 5 + 1];
        assert!(rc.put_chunk(0, 0, &body));
        let err = rc.build_file_checked().expect_err("256 sectors は拒否");
        assert!(err.contains("255-sector"), "上限由来の Err のはず: {err}");
        let mut rc2 = RegionCodec::new(CodecChoice::Stored);
        assert!(rc2.put_chunk(0, 0, &body));
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = rc2.build_file();
        }));
        assert!(r.is_err(), "build_file は fail-loud panic のはず");
    }

    /// wave 99 CX-2: scan_file の strict 検査 — (a) ヘッダ内を指す offset は
    /// InvalidData、(b) file 境界超過スパンも InvalidData、(c) 正規 file は受理
    /// (回帰ピンは file_layout_is_sector_aligned が担う)。
    #[test]
    fn scan_file_rejects_header_overlap_and_overspan() {
        let mut rc = RegionCodec::new(CodecChoice::ZstdFast);
        rc.put_chunk(0, 0, &sample_chunk(5));
        rc.put_chunk(1, 0, &sample_chunk(6));
        let good = rc.build_file();
        let scan = scan_file(&good).unwrap();
        assert_eq!(scan.chunks, 2, "前提: 正規 file は受理");
        // (a) offset=1 (ヘッダ 2 セクタ内) + sectors=1
        let mut bad_hdr = good.clone();
        bad_hdr[0..4].copy_from_slice(&((1u32 << 8) | 1).to_be_bytes());
        let e = scan_file(&bad_hdr).expect_err("ヘッダ重複は拒否");
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
        assert!(e.to_string().contains("header"));
        // (b) offset=2 + sectors=255 → span end = 257*4096 = 1,052,672 > file.len()
        let mut bad_span = good.clone();
        bad_span[0..4].copy_from_slice(&((2u32 << 8) | 0xFF).to_be_bytes());
        let e = scan_file(&bad_span).expect_err("境界超過は拒否");
        assert!(e.to_string().contains("exceeds"));
        // 機械検算: span end = (2+255)*4096
        assert_eq!((2 + 255) * SECTOR, 1_052_672);
    }
}
