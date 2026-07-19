// GPU frustum cull → D3D12_DRAW_ARGUMENTS for ExecuteIndirect.
// Layout matches D3D12_DRAW_ARGUMENTS (4 × uint).

cbuffer CullCB : register(b0)
{
    float4 frustum_planes[6]; // xyz = normal, w = distance
    uint instance_count;
    uint vertex_count_per_instance;
    uint2 _pad;
};

StructuredBuffer<float4> g_aabbs : register(t0); // xyz = center, w = radius
RWStructuredBuffer<uint4> g_draw_args : register(u0);

bool sphere_in_frustum(float3 c, float r)
{
    [unroll]
    for (uint i = 0; i < 6; i++)
    {
        if (dot(frustum_planes[i].xyz, c) + frustum_planes[i].w < -r)
            return false;
    }
    return true;
}

[numthreads(64, 1, 1)]
void CsMain(uint3 dtid : SV_DispatchThreadID)
{
    uint id = dtid.x;
    if (id >= instance_count)
        return;

    float4 aabb = g_aabbs[id];
    bool visible = sphere_in_frustum(aabb.xyz, aabb.w);

    // D3D12_DRAW_ARGUMENTS
    uint4 args;
    args.x = visible ? vertex_count_per_instance : 0; // VertexCountPerInstance
    args.y = visible ? 1u : 0u;                       // InstanceCount
    args.z = 0;                                       // StartVertexLocation
    args.w = id;                                      // StartInstanceLocation
    g_draw_args[id] = args;
}
