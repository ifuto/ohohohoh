// Frustum AABB culling over SoA boxes. On the GPU this becomes a SIMD/compute
// pass; the CPU reference lives in simd_frustum.rs.
