// Vertex cache optimization is a CPU/index-reordering pass; this WGSL note documents
// the runtime benefit: an optimized index buffer maximizes post-transform cache hits
// so the vertex shader runs ~1.2x per triangle instead of 3x. No GPU shader needed.

fn CacheScore(cachePos: i32) -> f32 {
    if (cachePos < 0) { return 0.75; }
    if (cachePos < 3) { return 1.0 / (f32(cachePos) + 1.0); }
    return 2.0 / (f32(cachePos) + 2.0);
}
