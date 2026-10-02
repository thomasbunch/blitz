// Instanced, solid-coloured rectangles in pixel coordinates.

cbuffer Frame : register(b0)
{
    float2 inv_half_size; // 2 / render target size in pixels
};

struct Quad
{
    float4 rect : RECT;   // x, y, width, height in pixels
    float4 color : COLOR; // premultiplied
};

struct VsOut
{
    float4 pos : SV_Position;
    float4 color : COLOR;
};

VsOut vs_main(Quad q, uint id : SV_VertexID)
{
    float2 corner = float2(id & 1, id >> 1);
    float2 px = q.rect.xy + corner * q.rect.zw;
    VsOut o;
    o.pos = float4(px * inv_half_size * float2(1, -1) + float2(-1, 1), 0, 1);
    o.color = q.color;
    return o;
}

float4 ps_main(VsOut i) : SV_Target
{
    return i.color;
}
