// Instanced quads in pixel coordinates: solid rectangles, and glyphs whose
// coverage comes from the R8 atlas.
//
// The glyph contrast and gamma correction is adapted from Windows Terminal's
// dwrite_helpers.hlsl (DWrite_EnhanceContrast, DWrite_ApplyAlphaCorrection):
// Copyright (c) Microsoft Corporation. Licensed under the MIT License.

cbuffer Frame : register(b0)
{
    float2 inv_half_size;   // 2 / render target size in pixels
    float enhanced_contrast;
    float pad;
    float4 gamma_ratios;
};

Texture2D<float> atlas : register(t0);

struct Quad
{
    int2 pos : POS;      // top-left, pixels
    uint2 size : SIZE;   // pixels
    uint2 uv : UV;       // atlas top-left, pixels
    float4 color : COLOR; // straight alpha
    uint flags : FLAGS;  // 0 solid, 1 font glyph, 2 exact coverage mask
};

struct VsOut
{
    float4 pos : SV_Position;
    float2 uv : UV;
    nointerpolation float4 color : COLOR;
    nointerpolation uint flags : FLAGS;
};

VsOut vs_main(Quad q, uint id : SV_VertexID)
{
    float2 corner = float2(id & 1, id >> 1);
    float2 px = q.pos + corner * q.size;
    VsOut o;
    o.pos = float4(px * inv_half_size * float2(1, -1) + float2(-1, 1), 0, 1);
    o.uv = q.uv + corner * q.size;
    o.color = q.color;
    o.flags = q.flags;
    return o;
}

float enhance_contrast(float alpha, float k)
{
    return alpha * (k + 1.0f) / (alpha * k + 1.0f);
}

float apply_alpha_correction(float a, float f, float4 g)
{
    return a + a * (1.0f - a) * ((g.x * f + g.y) * a + (g.z * f + g.w));
}

float4 ps_main(VsOut i) : SV_Target
{
    float a = i.color.a;
    if (i.flags != 0)
    {
        float cov = atlas.Load(int3(i.uv, 0));
        if (i.flags == 1)
        {
            float3 c = i.color.rgb;
            // Light text on a dark background gets less contrast boost.
            float k = enhanced_contrast * saturate(dot(c, float3(0.30f, 0.59f, 0.11f)) * -4.0f + 3.0f);
            cov = enhance_contrast(cov, k);
            cov = apply_alpha_correction(cov, dot(c, float3(0.25f, 0.5f, 0.25f)), gamma_ratios);
        }
        a *= saturate(cov);
    }
    return float4(i.color.rgb * a, a);
}
