// SFS MinMip resolve — decode feedback into tile request flags.
// numthreads 8x8 keeps occupancy high on Intel iGPU (GDC Sampler Feedback guidance).
Texture2D<uint> g_feedback : register(t0); // R8_UINT after DECODE_SAMPLER_FEEDBACK
RWStructuredBuffer<uint> g_tile_requests : register(u0);
RWStructuredBuffer<uint> g_request_count : register(u1);

cbuffer SfsCB : register(b0)
{
    uint feedback_width;
    uint feedback_height;
    uint max_tiles;      // CPU budget (8 on low-spec)
    uint unused_mip;     // 0xFF
};

[numthreads(8, 8, 1)]
void CsMain(uint3 dtid : SV_DispatchThreadID)
{
    if (dtid.x >= feedback_width || dtid.y >= feedback_height)
        return;

    uint mip = g_feedback.Load(int3(dtid.xy, 0));
    if (mip == unused_mip)
        return;

    // Coalesce 2x2 feedback texels into one tile index.
    uint tile_x = dtid.x >> 1;
    uint tile_y = dtid.y >> 1;
    uint tiles_per_row = (feedback_width + 1) >> 1;
    uint tile = tile_y * tiles_per_row + tile_x;

    // Atomic flag — first writer wins; CPU reads sparse list later.
    uint prev;
    InterlockedExchange(g_tile_requests[tile], 1, prev);
    if (prev == 0)
    {
        uint idx;
        InterlockedAdd(g_request_count[0], 1, idx);
        // Optional compact list in upper half of buffer when under budget.
        if (idx < max_tiles)
            g_tile_requests[max_tiles + idx] = tile | (mip << 16);
    }
}
