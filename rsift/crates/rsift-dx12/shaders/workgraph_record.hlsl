// Work Graph record node — enqueue visible quads for expansion (SM 6.8+ node shader).

[Shader("node")]
[NodeLaunch("broadcasting")]
void RecordMain(
    uint3 gid : SV_GroupID,
    uint dtid : SV_DispatchThreadID)
{
    // Write quad indices into work graph payload queue.
}
