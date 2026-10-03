// Instanced quads in pixel coordinates: solid rectangles, glyphs whose
// coverage comes from the R8 atlas, and curly, dotted and dashed lines.
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
    uint2 uv : UV;       // atlas top-left, pixels; for lines, the cell
                         // width and the line thickness
    float4 color : COLOR; // straight alpha
    uint flags : FLAGS;  // 0 solid, 1 font glyph, 2 exact coverage mask,
                         // 3 curly, 4 dotted, 5 dashed line
};

struct VsOut
{
    float4 pos : SV_Position;
    float2 uv : UV;
    nointerpolation float2 base : BASE; // the quad's uv
    nointerpolation float2 size : SIZE;
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
    o.base = q.uv;
    o.size = q.size;
    o.color = q.color;
    o.flags = q.flags;
    return o;
}

// Coverage of a curly (3), dotted (4) or dashed (5) line at `p` in a box
// of `size`, for cells `cell` pixels wide and lines `t` thick. Each
// pattern repeats a whole number of times per cell, so the lines of
// neighbouring cells join up.
float line_coverage(uint kind, float2 p, float2 size, float cell, float t)
{
    if (kind == 3)
    {
        // One wave per cell, from the bottom of the box to the top.
        float amp = (size.y - t) * 0.5f;
        float k = 6.2831853f / cell;
        float y = size.y * 0.5f + amp * cos(p.x * k);
        float slope = amp * k * sin(p.x * k);
        // Distance to the curve, near enough, against half the thickness.
        float d = abs(p.y - y) / sqrt(1.0f + slope * slope);
        return saturate(t * 0.5f + 0.5f - d);
    }
    // Dots as long as the line is thick with gaps as long, or dashes
    // four times that with gaps half as long.
    float period = kind == 4 ? 2.0f * t : 6.0f * t;
    float n = max(1.0f, floor(cell / period + 0.5f));
    float on = kind == 4 ? 0.5f : 2.0f / 3.0f;
    return frac(p.x * n / cell) < on ? 1.0f : 0.0f;
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
    if (i.flags >= 3)
    {
        a *= line_coverage(i.flags, i.uv - i.base, i.size, i.base.x, i.base.y);
    }
    else if (i.flags != 0)
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
