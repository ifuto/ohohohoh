//! Forsyth "Linear-Speed Vertex Cache Optimisation".
//!
//! Reorders an index buffer to maximise post-transform vertex cache hits.
//! Lower ACMR (average cache misses per triangle) means the vertex shader
//! runs fewer times — a direct, bandwidth-free speed-up that especially
//! helps integrated GPUs with small vertex caches.

#[derive(Debug, Clone, Copy)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}
impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
}
impl std::ops::Add for Vec3 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl std::ops::Sub for Vec3 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl std::ops::Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s)
    }
}

const CACHE_SIZE: usize = 16;
const CACHE_DECAY_POWER: f32 = 1.5;
const VALENCE_BOOST: f32 = 2.0;
const VALENCE_DECAY_POWER: f32 = 2.0;

/// Score of a single vertex given its cache position (-1 = not cached) and
/// remaining valence (number of not-yet-emitted triangles referencing it).
fn vertex_score(cache_position: i32, valence: u32) -> f32 {
    if valence == 0 {
        return -1.0;
    }
    let mut score = 0.0_f32;
    if cache_position >= 0 {
        if cache_position < 3 {
            let p = cache_position as f32;
            score += (1.0 - p / 3.0).powf(CACHE_DECAY_POWER) * 2.0;
        } else {
            let p = cache_position as f32;
            let tail = (CACHE_SIZE as f32 - 1.0 - p) / (CACHE_SIZE as f32 - 1.0);
            score += (1.0 - p / (CACHE_SIZE as f32)).powf(CACHE_DECAY_POWER) * 2.0 * tail;
        }
    }
    let v = valence as f32;
    score += (1.0 / v).powf(VALENCE_DECAY_POWER) * VALENCE_BOOST;
    score
}

fn tri_score(t: usize, indices: &[u32], live: &[u32], pos_of: &[i32]) -> f32 {
    let mut s = 0.0_f32;
    for k in 0..3 {
        let v = indices[t * 3 + k] as usize;
        s += vertex_score(pos_of[v], live[v]);
    }
    s
}

fn cache_touch(cache: &mut Vec<i32>, pos_of: &mut [i32], v: i32) {
    if let Some(p) = cache.iter().position(|&x| x == v) {
        cache.remove(p);
    }
    cache.insert(0, v);
    if cache.len() > CACHE_SIZE {
        cache.pop();
    }
    for e in pos_of.iter_mut() {
        *e = -1;
    }
    for (i, &x) in cache.iter().enumerate() {
        if x >= 0 {
            pos_of[x as usize] = i as i32;
        }
    }
}

/// Optimize an index buffer (groups of 3 vertex indices) for vertex cache
/// reuse. `vertex_count` is the number of distinct vertices.
pub fn optimize(indices: &[u32], vertex_count: usize) -> Vec<u32> {
    let tri_count = indices.len() / 3;

    let mut live_triangles = vec![0u32; vertex_count];
    let mut vert_to_tri: Vec<Vec<usize>> = vec![Vec::new(); vertex_count];
    for t in 0..tri_count {
        for k in 0..3 {
            let v = indices[t * 3 + k] as usize;
            live_triangles[v] += 1;
            vert_to_tri[v].push(t);
        }
    }

    let mut triangle_scores = vec![0.0f32; tri_count];
    let mut emitted = vec![false; tri_count];
    let mut cache: Vec<i32> = Vec::with_capacity(CACHE_SIZE);
    let mut pos_of = vec![-1i32; vertex_count];

    for t in 0..tri_count {
        triangle_scores[t] = tri_score(t, indices, &live_triangles, &pos_of);
    }

    let mut output = Vec::with_capacity(indices.len());
    let mut emitted_count = 0;
    while emitted_count < tri_count {
        // Pick the highest-scoring not-yet-emitted triangle.
        let mut best = 0usize;
        let mut best_score = f32::NEG_INFINITY;
        for t in 0..tri_count {
            if !emitted[t] && triangle_scores[t] > best_score {
                best_score = triangle_scores[t];
                best = t;
            }
        }

        emitted[best] = true;
        for k in 0..3 {
            output.push(indices[best * 3 + k]);
        }
        emitted_count += 1;

        // Decrement liveness, then push this triangle's vertices into the cache.
        for k in 0..3 {
            let v = indices[best * 3 + k] as usize;
            live_triangles[v] -= 1;
        }
        for k in 0..3 {
            let v = indices[best * 3 + k] as i32;
            cache_touch(&mut cache, &mut pos_of, v);
        }
        // Re-score triangles adjacent to the emitted vertices.
        for k in 0..3 {
            let v = indices[best * 3 + k] as usize;
            for &tt in &vert_to_tri[v] {
                if !emitted[tt] {
                    triangle_scores[tt] = tri_score(tt, indices, &live_triangles, &pos_of);
                }
            }
        }
    }

    output
}

pub struct VertexCacheOptimizer;
impl VertexCacheOptimizer {
    pub fn wgsl_source(&self) -> &'static str {
        VERTEX_CACHE_OPT_WGSL
    }
}

pub const VERTEX_CACHE_OPT_WGSL: &str = include_str!("../shaders/vertex_cache_opt.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    /// Average Cache Miss Rate: vertex-shader invocations per triangle.
    fn acmr(indices: &[u32], cache_size: usize) -> f32 {
        let tri = indices.len() / 3;
        let mut cache: Vec<u32> = Vec::new();
        let mut invocations = 0u32;
        for t in 0..tri {
            for k in 0..3 {
                let v = indices[t * 3 + k];
                if !cache.contains(&v) {
                    invocations += 1;
                    cache.insert(0, v);
                    if cache.len() > cache_size {
                        cache.pop();
                    }
                }
            }
        }
        invocations as f32 / tri as f32
    }

    #[test]
    fn output_is_a_permutation() {
        // Two triangles sharing one edge, third referencing distant vertices.
        let indices: Vec<u32> = vec![
            0, 1, 2, // tri A
            2, 1, 3, // tri B (shares edge 1-2)
            4, 5, 6, // tri C (far away)
            6, 5, 7, // tri D
        ];
        let out = optimize(&indices, 8);
        assert_eq!(out.len(), indices.len());
        let mut a = indices.clone();
        let mut b = out.clone();
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b, "optimized index buffer must be a permutation");
    }

    #[test]
    fn acmr_does_not_get_worse() {
        // Long strip-like mesh with poor naive ordering.
        let mut indices = Vec::new();
        for i in 0..200u32 {
            indices.push(i);
            indices.push(i + 1);
            indices.push(i + 2);
        }
        // Shuffle to create a bad initial order.
        let mut shuffled = indices.clone();
        for i in (0..shuffled.len()).rev() {
            shuffled.swap(i, i % 7);
        }
        let before = acmr(&shuffled, 16);
        let after = acmr(&optimize(&shuffled, 202), 16);
        assert!(
            after <= before + 1e-3,
            "optimization should not increase ACMR (before={}, after={})",
            before,
            after
        );
    }
}
