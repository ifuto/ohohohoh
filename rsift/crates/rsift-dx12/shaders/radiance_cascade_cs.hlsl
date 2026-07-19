// Screen-space Radiance Cascades probe merge (Sannikov / GM Shaders RC — simplified).
// Cascade 0 samples neighborhood luminance into probe atlas UAV.

Texture2D<float4> g_color : register(t0);
RWStructuredBuffer<float4> g_probes : register(u0);

cbuffer RcCB : register(b0)
{
    uint screen_w;
    uint screen_h;
    uint grid_w;
    uint grid_h;
    uint rays_per_probe;
    uint cascade_index;
    float2 _pad;
};

[numthreads(8, 8, 1)]
void CsMain(uint3 dtid : SV_DispatchThreadID)
{
    if (dtid.x >= grid_w || dtid.y >= grid_h)
        return;

    float2 uv = (float2(dtid.xy) + 0.5) / float2(grid_w, grid_h);
    int2 px = int2(uv * float2(screen_w, screen_h));
    px = clamp(px, int2(0, 0), int2(screen_w - 1, screen_h - 1));

    float3 sum = 0;
    uint samples = max(rays_per_probe, 1);
    float radius = float(1u << cascade_index) * 4.0;
    for (uint i = 0; i < samples; i++)
    {
        float ang = (float(i) + 0.5) / float(samples) * 6.2831853;
        int2 q = px + int2(cos(ang) * radius, sin(ang) * radius);
        q = clamp(q, int2(0, 0), int2(screen_w - 1, screen_h - 1));
        sum += g_color.Load(int3(q, 0)).rgb;
    }
    sum /= float(samples);

    uint probe_id = dtid.y * grid_w + dtid.x;
    // Direction-first packing: one float4 per probe at this cascade (averaged radiance).
    g_probes[probe_id + cascade_index * grid_w * grid_h] = float4(sum, 1.0);
}
