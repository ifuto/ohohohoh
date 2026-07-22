// CPU-side texture streaming (VRAM budget + LRU + hysteresis). No GPU shader
// required; the residency decision runs on the CPU at load/stream time.
//
// これはスタブではなく、設計上シェーダが存在し得ないことの正当なマーカー
// (2026-07-23 wave 43 で正当理由を強化):
//   1. 常駐/退避の決定は VRAM リソースの生成・破棄・上傳を伴うホスト API
//      操作であり、GPU シェーダはリソース割当を行えない。
//   2. 退避→上傳→ページテーブル更新の順序はシェーダ実行「前」に確定
//      している必要があるため、決定を GPU 側へ遅延させると 1 フレーム
//      遅れの自己参照ループになる。
// このファイルは空モジュールとして naga parse 可能であること、および
// entry/binding を持たないことが src/mip_streaming.rs のテスト
// (wgsl_is_intentionally_shader_free_marker) で機械ピンされている。
