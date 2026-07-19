// Vertex pull terrain — no input layout, Reverse-Z.

cbuffer FrameCB : register(b0)
{
    float4x4 view_proj;
    float4 chunk_origin;
};

StructuredBuffer<uint2> g_quads : register(t0);

static const uint TRI[6] = { 0, 1, 2, 2, 3, 0 };

float3 corner_pos(uint face, float3 o, float w, float h, uint c)
{
    float2 uv = float2(c == 1 || c == 2, c >= 2);
    switch (face) {
        case 0: return float3(o.x + 1, o.y + uv.y * h, o.z + uv.x * w);
        case 1: return float3(o.x, o.y + uv.y * h, o.z + uv.x * w);
        case 2: return float3(o.x + uv.x * w, o.y + 1, o.z + uv.y * h);
        case 3: return float3(o.x + uv.x * w, o.y, o.z + uv.y * h);
        case 4: return float3(o.x + uv.x * w, o.y + uv.y * h, o.z + 1);
        default: return float3(o.x + uv.x * w, o.y + uv.y * h, o.z);
    }
}

struct VsOut {
    float4 pos : SV_Position;
    float2 uv : TEXCOORD0;
    uint tex_id : TEXCOORD1;
    uint quad_id : TEXCOORD2;
};

VsOut VsMain(uint vid : SV_VertexID)
{
    uint qid = vid / 6;
    uint corner = TRI[vid % 6];
    // One PackedPullQuad = one uint2 (8 bytes).
    uint2 q = g_quads[qid];

    uint x = q.x & 63;
    uint y = (q.x >> 6) & 63;
    uint z = (q.x >> 12) & 63;
    uint tex = (q.x >> 18) & 4095;
    uint face = q.y & 7;
    float w = float(((q.y >> 3) & 63) + 1);
    float h = float(((q.y >> 9) & 63) + 1);

    float3 local = corner_pos(face, float3(x, y, z), w, h, corner);
    float3 world = local + chunk_origin.xyz;

    VsOut o;
    o.pos = mul(float4(world, 1), view_proj);
    o.uv = float2(corner == 1 || corner == 2, corner >= 2);
    o.tex_id = tex;
    o.quad_id = qid;
    return o;
}
