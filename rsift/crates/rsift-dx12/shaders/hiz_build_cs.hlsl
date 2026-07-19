// Hi-Z mip chain build: each dispatch reduces previous mip 2×2 → min depth (Reverse-Z: max).

Texture2D<float> g_src : register(t0);
RWTexture2D<float> g_dst : register(u0);

cbuffer HizCB : register(b0)
{
    uint src_width;
    uint src_height;
    uint dst_width;
    uint dst_height;
};

[numthreads(8, 8, 1)]
void CsMain(uint3 dtid : SV_DispatchThreadID)
{
    if (dtid.x >= dst_width || dtid.y >= dst_height)
        return;

    // Same-size pass seeds Hi-Z mip0 from depth (or previous) without downsample.
    if (src_width == dst_width && src_height == dst_height)
    {
        g_dst[dtid.xy] = g_src.Load(int3(dtid.xy, 0));
        return;
    }

    uint2 s = uint2(dtid.xy) * 2;
    float d0 = g_src.Load(int3(min(s.x, src_width - 1), min(s.y, src_height - 1), 0));
    float d1 = g_src.Load(int3(min(s.x + 1, src_width - 1), min(s.y, src_height - 1), 0));
    float d2 = g_src.Load(int3(min(s.x, src_width - 1), min(s.y + 1, src_height - 1), 0));
    float d3 = g_src.Load(int3(min(s.x + 1, src_width - 1), min(s.y + 1, src_height - 1), 0));
    // Reverse-Z: farther = smaller; conservative occlusion uses min of children.
    g_dst[dtid.xy] = min(min(d0, d1), min(d2, d3));
}
