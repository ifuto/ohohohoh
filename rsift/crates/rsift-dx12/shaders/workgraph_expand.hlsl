// Work Graph mesh expansion node — 6 verts per quad, no IBO (SM 6.8+ mesh shader).

[Shader("mesh")]
[NodeLaunch("broadcasting")]
void ExpandMain(
    uint3 gid : SV_GroupID,
    uint dtid : SV_DispatchThreadID)
{
    SetMeshOutputCounts(6, 2);
}
