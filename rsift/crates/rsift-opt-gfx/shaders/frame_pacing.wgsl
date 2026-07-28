// CPU-side frame pacing (vsync snap + EMA smoothing). No GPU shader required;
// the timing logic runs on the CPU before each present.
//
// これはスタブではなく、設計上シェーダが存在し得ないことの正当なマーカー
// (2026-07-28 wave 161 で mip_streaming wave 43 様式へ強化):
//   1. 提示時刻の決定は swapchain 提出 (ホスト API 呼び出し) の「前」に行う
//      CPU 計時であり、GPU シェーダは提示そのもののスケジュールを行えない
//      (決定を GPU へ送る提出自体が既に「提示」である自己参照ループ)。
//   2. vsync 境界はホスト側のタイムベースで管理され、シェーダからは観測不能。
// このファイルは空モジュールとして naga parse 可能であること、および
// entry/binding を持たないことが src/frame_pacing.rs のテスト
// (wgsl_is_intentionally_shader_free_marker) で機械ピンされている。
