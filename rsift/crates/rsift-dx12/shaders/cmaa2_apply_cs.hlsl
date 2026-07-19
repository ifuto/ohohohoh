// CMAA2-inspired deferred edge blend apply.

Texture2D<float4> g_color_in : register(t0);
StructuredBuffer<uint> g_edges : register(t1);
RWTexture2D<float4> g_color_out : register(u0);

cbuffer CmaaCB : register(b0)
{
    uint width;
    uint height;
    float blend;
    uint _pad;
};

[numthreads(8, 8, 1)]
void CsMain(uint3 dtid : SV_DispatchThreadID)
{
    if (dtid.x >= width || dtid.y >= height)
        return;

    float4 c = g_color_in.Load(int3(dtid.xy, 0));
    uint flags = g_edges[dtid.y * width + dtid.x];
    if (flags != 0)
    {
        float4 acc = c;
        uint n = 1;
        if (flags & 1)
        {
            acc += g_color_in.Load(int3(min(dtid.x + 1, width - 1), dtid.y, 0));
            n++;
        }
        if (flags & 2)
        {
            acc += g_color_in.Load(int3(dtid.x, min(dtid.y + 1, height - 1), 0));
            n++;
        }
        c = lerp(c, acc / float(n), blend);
    }
    g_color_out[dtid.xy] = c;
}
