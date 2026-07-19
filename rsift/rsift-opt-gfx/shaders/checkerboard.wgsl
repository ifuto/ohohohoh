// Checkerboard reconstruction: fill the unrendered (odd) pixels by averaging
// the four rendered diagonal neighbours.

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var dst: texture_storage_2d<rgba8unorm, write>;

fn rendered(x: i32, y: i32) -> bool {
    (x + y) % 2 == 0
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dim = textureDimensions(src);
    if (gid.x >= dim.x || gid.y >= dim.y) {
        return;
    }
    let x = i32(gid.x);
    let y = i32(gid.y);
    if (rendered(x, y)) {
        return; // this pixel was already shaded this frame
    }
    let nw = textureLoad(src, vec2<i32>(x - 1, y - 1), 0).rgb;
    let ne = textureLoad(src, vec2<i32>(x + 1, y - 1), 0).rgb;
    let sw = textureLoad(src, vec2<i32>(x - 1, y + 1), 0).rgb;
    let se = textureLoad(src, vec2<i32>(x + 1, y + 1), 0).rgb;
    let avg = (nw + ne + sw + se) * 0.25;
    textureStore(dst, vec2<i32>(x, y), vec4<f32>(avg, 1.0));
}
