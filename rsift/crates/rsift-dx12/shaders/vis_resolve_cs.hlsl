// Material resolve from visibility buffer IDs.

Texture2D<uint> g_vis : register(t0);
RWTexture2D<float4> g_color : register(u0);

cbuffer ResolveCB : register(b0)
{
    uint width;
    uint height;
    uint2 _pad;
};

[numthreads(8, 8, 1)]
void CsMain(uint3 dtid : SV_DispatchThreadID)
{
    if (dtid.x >= width || dtid.y >= height)
        return;

    uint packed = g_vis.Load(int3(dtid.xy, 0));
    if (packed == 0xFFFFFFFFu)
    {
        g_color[dtid.xy] = float4(0.05, 0.15, 0.28, 1.0);
        return;
    }
    uint tex = packed >> 16;
    float band = float(tex % 7) / 7.0;
    // Slightly brighter resolve path so VisBuffer contribution is visible.
    g_color[dtid.xy] = float4(band * 1.05, band * 0.65, band * 0.35, 1.0);
}
