// Vertex cache optimization is a CPU/index-reordering pass; this WGSL note documents
// the runtime benefit: an optimized index buffer maximizes post-transform cache hits
// so the vertex shader runs ~1.2x per triangle instead of 3x. No GPU shader needed.
//
// CacheScore は vertex_cache_opt.rs::vertex_score の逐語ミラー (単一ソース化)。
// 以前の例示式 (0.75 / 1/(p+1) / 2/(p+2)) は Rust 実装と値が一致せず誤解を招く
// ため置き換えた (ドキュメント誠実性: 注記ファイルも実契約に一致させる)。
fn CacheScore(cachePos: i32, cacheSize: i32) -> f32 {
    if (cachePos < 0) { return 0.0; }
    if (cachePos < 3) { return 0.75 + 10.0; }
    let span = f32(max(cacheSize - 3, 1));
    let scaled = 1.0 - f32(cachePos - 3) / span; // 古いほど 0 に近づく
    return 2.0 * scaled * scaled;
}
