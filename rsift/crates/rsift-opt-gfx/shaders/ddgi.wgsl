// DDGI probe volume — octahedral encode + Chebyshev visibility.
// Companion to src/ddgi.rs.

fn octEncode(n : vec3<f32>) -> vec2<f32> {
    let s = max(abs(n.x) + abs(n.y) + abs(n.z), 1e-8);
    var o = vec2<f32>(n.x / s, n.y / s);
    if (n.z < 0.0) {
        o = vec2<f32>(
            (1.0 - abs(o.y)) * select(-1.0, 1.0, o.x >= 0.0),
            (1.0 - abs(o.x)) * select(-1.0, 1.0, o.y >= 0.0));
    }
    return o;
}

fn octDecode(f : vec2<f32>) -> vec3<f32> {
    var n = vec3<f32>(f.x, f.y, 1.0 - abs(f.x) - abs(f.y));
    if (n.z < 0.0) {
        n = vec3<f32>(
            (1.0 - abs(f.y)) * select(-1.0, 1.0, f.x >= 0.0),
            (1.0 - abs(f.x)) * select(-1.0, 1.0, f.y >= 0.0),
            n.z);
    }
    return normalize(n);
}

fn chebyshev(m1 : f32, m2 : f32, d : f32) -> f32 {
    if (d <= m1) { return 1.0; }
    let variance = max(m2 - m1 * m1, 0.0);
    let diff = d - m1;
    return clamp(variance / (variance + diff * diff), 0.0, 1.0);
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid : vec3<u32>) {
    let _ = gid;
    // Octahedral encode/decode + Chebyshev are invoked per probe/texel on GPU.
}
