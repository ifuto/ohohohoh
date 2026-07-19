// Visibility buffer write — pack quad id + tex id (JCGT VisBuffer / arXiv:2204.01287).

struct PsIn {
    float4 pos : SV_Position;
    float2 uv : TEXCOORD0;
    uint tex_id : TEXCOORD1;
    uint quad_id : TEXCOORD2;
};

struct PsOut {
    float4 color : SV_Target0;
    uint vis : SV_Target1;
};

PsOut PsMain(PsIn i)
{
    PsOut o;
    float band = float(i.tex_id % 7) / 7.0;
    o.color = float4(band, band * 0.6, band * 0.3, 1.0);
    o.vis = (i.tex_id << 16) | (i.quad_id & 0xFFFFu);
    return o;
}
