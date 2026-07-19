// CMAA2-inspired edge detect (Intel morphological AA — quality preset 0 path).

Texture2D<float4> g_color : register(t0);
RWStructuredBuffer<uint> g_edges : register(u0);

cbuffer CmaaCB : register(b0)
{
    uint width;
    uint height;
    float edge_threshold;
    uint _pad;
};

float luma(float3 c)
{
    return dot(c, float3(0.299, 0.587, 0.114));
}

[numthreads(8, 8, 1)]
void CsMain(uint3 dtid : SV_DispatchThreadID)
{
    if (dtid.x >= width || dtid.y >= height)
        return;

    float3 c = g_color.Load(int3(dtid.xy, 0)).rgb;
    float3 r = g_color.Load(int3(min(dtid.x + 1, width - 1), dtid.y, 0)).rgb;
    float3 d = g_color.Load(int3(dtid.x, min(dtid.y + 1, height - 1), 0)).rgb;

    float lc = luma(c);
    uint flags = 0;
    if (abs(lc - luma(r)) > edge_threshold)
        flags |= 1;
    if (abs(lc - luma(d)) > edge_threshold)
        flags |= 2;
    g_edges[dtid.y * width + dtid.x] = flags;
}
