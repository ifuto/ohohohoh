// Compute emulation of task-shader meshlet dispatch when mesh_shader unavailable.
// Writes indirect draw args: (vertex_count = quads*6, instance_count = 1).

struct PullQuad {
    word0: u32,
    word1: u32,
}

struct DrawArgs {
    vertex_count: u32,
    instance_count: u32,
    first_vertex: u32,
    first_instance: u32,
}

struct DispatchParams {
    quad_count: u32,
    meshlet_size: u32,
    chunk_slot: u32,
    _pad: u32,
}

@group(0) @binding(0) var<uniform> params: DispatchParams;
@group(0) @binding(1) var<storage, read> quads: array<PullQuad>;
@group(0) @binding(2) var<storage, read_write> draw_args: array<DrawArgs>;

@compute @workgroup_size(64, 1, 1)
fn cs_task_emulation(@builtin(global_invocation_id) gid: vec3<u32>) {
    let meshlet = gid.x;
    let base = meshlet * params.meshlet_size;
    if base >= params.quad_count {
        return;
    }
    var count = params.meshlet_size;
    if base + count > params.quad_count {
        count = params.quad_count - base;
    }
    draw_args[meshlet].vertex_count = count * 6u;
    draw_args[meshlet].instance_count = 1u;
    draw_args[meshlet].first_vertex = base * 6u;
    draw_args[meshlet].first_instance = params.chunk_slot;
}
