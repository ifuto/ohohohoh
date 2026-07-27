# RsReplay exporter モーションブラー監査レポート — 2026-07-28 (wave 146)

**対象: arena/019f88d7-rsift / crates/rsift-replay (exporter.rs・renderer.rs) /
実施: Arena エージェント (ローカル Rust 1.94.1 実機械検証・全数値 rq 事前導出)**

opt-gfx 系連続監査 (AUDIT_2026-07-21_RSGFX_MODULES.md) の census grep 中に
検出された §7 違反 (空関数スタブ + 消費者ゼロ設定フィールド) の専用修繕 wave。
節純度のため本 wave は rsift-replay のみを対象とする。

## 検出の経緯 (機械確定)

- `crates/rsift-replay/src/exporter.rs:145`:
  `pub fn apply_motion_blur(_frames: &mut [RenderedFrame], _strength: f32) {}`
  — **空関数スタブ**。census grep (workspace 全域) で呼出消費者ゼロ。
- 同 `ExportSettings.motion_blur: f32` (既定 0.0): 定義と Default 以外の
  参照ゼロ (§7「実装はしたものの未配線」相当の消費者なしフィールド)。
- 代替実体: renderer.rs `MotionBlurAccumulator` (f64 蓄積) +
  `OfflineRenderSettings.motion_blur_samples` は
  `render_first_person_frame` が実消費 (subframe 位相累積の真 shutter)。
  FFI `rsreplay_export_mp4` (mods-official/rsreplay/src/lib.rs:166-167) は
  renderer 経路のみ配線 → exporter 側 strength は全経路で変更されない。

## RP-1 [中] スタブ根治 (§7: 実消費者追加配線を第一選択として実施)

- `blur_radius(strength)`: round(clamp(strength,0,1)×4) → 0..=4 (MAX_BLUR_RADIUS)。
  半値持ち上げ (f32::round = half away from zero: 0.125→1 を golden 化)。
  非有限 strength は fail-loud panic (旧スタブは全入力を静寂呑み)。
- `apply_motion_blur`: 出力 out[i] = mean(frames[max(0,i-r)..=min(n-1,i+r)])
  の箱型一様重み。端は利用フレームのみで再正規化 (重み和 1 維持)。
  丸めは整数 half-up `(sum + count/2)/count`・全チャンネル同一規則。
  sum 最大 9×255=2295、(2295+4)/9=255 < 256 より u8 飽和 clamp 不要
  (rq assert で証明)。in-place でも汚染なし (生値全複製から書戻し)。
  空スライス・r=0 は no-op。不均一 dims は fail-loud panic。
- 配線: `Mp4Exporter::export()` が settings.motion_blur > 0 のとき
  blur 分岐へ。リングバッファに 2r+1 フレームのみ保持し、中心が揃い次第
  窓を複製→apply_motion_blur→中心 TGA 書出し (全フレーム滞留を避ける
  低スペック配慮)。末尾 r フレーム分は右端 clamp 窓でフラッシュ。
  出力総数は total_frames と一致 (主ループ n-r + 尾 r、n<r+1 退化も
  saturating で一意)。既定 0.0 は従来経路と byte 同一。
- 誠実注記 (fn doc/module doc に記載): 真 shutter (renderer 経路) が数学的
  上位で、post-hoc は区級近似。同時有効は二重ブラー。現行 FFI では
  exporter strength を変更しないため GUI 経由出力では非発火 (crate 直接
  利用向け独立制御)。窓全体を blur して中心のみ使う ≤9 倍の冗長計算を
  許容 (単一カーネル単一実装を優先)。

## RP-2 [中] 派生発見: write_tga 20B 非標準ヘッダ (潜在バグ)

wiring テストの TGA 読取設計で機械発見: ヘッダ前置ブロックが 14B
(標準 12B+2B 過剰) で全体 20B、width/height フィールドが標準位置から
2B ずれており、標準準拠の ffmpeg image2 TGA 復号では width=0 となり
復号不能。ffmpeg 非存在の現環境では try_ffmpeg_encode 内の別理由
(spawn 失敗) で先に Err となるため未顕在化していた。→ 18B 標準へ修正
+ 回帰防止 pin (ファイル長 18+64×64×3・width LE=64,0・bpp=24)。

## RP-3 [低] strict 12 件・fault-injection (adversarial)

- TDD 修正前 RED 8 件 (ramp r=1/r=2・rounding・contamination・NaN/不均一
  panic・wiring blur 5 画素 golden・既定オフ生パターン)→ 本実装で 16 全緑
  (replay lib: HEAD 3 + wave 12 + renderer 既存 1)。
- golden 源パターン: renderer 生成画素 r-ch = 3 ^ frame (64x64・(1,1) 画素)、
  blur r=2 の 5 出力 = [2,2,3,3,3] (端再正規化・half-up: (13+2)/5=3 等)。
  既定オフ時は生 [3,2,1,0,7] のまま (配線が既定経路を侵蝕しない pin)。
- fault-injection (実体コピー先行・毎回復元 MD5-VERIFIED 5 回):
  (a) half-up 除去 → **3 RED** (rounding・contamination 弁別・wiring golden)、
  (b) hi.clamp(n-1) 除去 → **7 RED** (OOB panic 系)、
  (c) lo.saturating_sub 除去 → **7 RED** (underflow panic 系)、
  (d) round→trunc → **1 RED** (半径写像 pin のみ = 補完正確)、
  (e) export 配線無効化 → **1 RED** (wiring blur golden のみ = 既定オフ pin
  は不変で正確)。**非検出ゼロ全 RED**。

## 機械値

- テスト: replay 16 全緑 (3.47-3.54s)・opt-gfx 1207 全緑 (21.32s)・api 49 全緑。
- rq `rp_mb.rq` 全 assert 通過 (半径写像 9 値・r=1/r=2 kernel golden・
  half-up 0.5→1・contaminated 弁別 67 vs 50・alpha invariant・overflow 証明
  2295+4<256×9、python 不使用)。
- 警告: lib test 5 件 (recorder/catalog: RSR_MAGIC/PacketRecordHeader/Path/
  format_duration/Path) = HEAD 原生 (git stash 前後同数で機械確認)。
- fmt: exporter.rs を rustfmt 正準化後に HEAD 原生逸脱 1 行 (warn! 呼出の
  折返し、line 154 系) のみ復元据置 → rustfmt --check 逸脱 1 行 = HEAD 包含。
  renderer.rs は HEAD 原生逸脱 11 行据置 (wave 非起因)。
- exporter.rs は CRLF (132 個) → LF へ全面正規化 (renderer.rs と同じ repo 標準
  LF に統一。混在行終端の事故誘発を根治)。
- 固定版 md5 三重保存: exporter.rs 35286f9276d6e9fb9d0a3b61fed8c5ae・
  renderer.rs c25ea718fcaf250f9c6ceb51f82ef123 (rsift/bak・/tmp・src 三重一致)。
- seal: 初回 2 FAIL (san: 私の混字 U+7EA7 1 件 → 級へ修正 2 箇所 (exporter doc + 本 md)・fmdiff: renderer 自己起因逸脱 2 行 → TGA 配列 1 行正準化) → 再 seal 全 6 ゲート PASS (san 0 findings・fmdiff 自己起因 0 (renderer 現逸脱 16 行 ≤ HEAD 20 行=包含・exporter 現 1 行=HEAD 原生据置)・trailws 0・digest 004c1cf5fb17bfe8 rows=357 不変、/tmp/w146_seal2.log 実測)・台帳 550・TRIGGER 183。
