// Compute fallback when Work Graphs unavailable (SM 6.6 path).

StructuredBuffer<uint2> g_quads : register(t0);
RWStructuredBuffer<uint4> g_expanded : register(u0);

[numthreads(64, 1, 1)]
void CsMain(uint3 dtid : SV_DispatchThreadID)
{
    uint qid = dtid.x;
    g_expanded[qid] = uint4(g_quads[qid * 2].x, g_quads[qid * 2].y, 0, 0);
}
