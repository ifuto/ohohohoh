// Production quad expand compute (mesh prep). Primary path when Work Graph SO is unavailable.

StructuredBuffer<uint2> g_quads : register(t0);
RWStructuredBuffer<uint4> g_expanded : register(u0);

[numthreads(64, 1, 1)]
void CsMain(uint3 dtid : SV_DispatchThreadID)
{
    uint qid = dtid.x;
    uint2 q = g_quads[qid];
    // Pack face metadata into expanded UAV for downstream mesh / debug consumers.
    g_expanded[qid] = uint4(q.x, q.y, qid, 1);
}
