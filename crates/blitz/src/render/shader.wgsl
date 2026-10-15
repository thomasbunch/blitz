// shader.hlsl for wgpu: instanced quads in pixel coordinates, solid
// rectangles, glyphs whose coverage comes from the R8 atlas, and curly,
// dotted and dashed lines. Keep the two in step.
//
// The glyph contrast and gamma correction is adapted from Windows Terminal's
// dwrite_helpers.hlsl (DWrite_EnhanceContrast, DWrite_ApplyAlphaCorrection):
// Copyright (c) Microsoft Corporation. Licensed under the MIT License.

struct Frame {
    inv_half_size: vec2<f32>, // 2 / render target size in pixels
    enhanced_contrast: f32,
    pad: f32,
    gamma_ratios: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var atlas: texture_2d<f32>;

struct Quad {
    @location(0) pos: vec2<i32>,   // top-left, pixels
    @location(1) size: vec2<u32>,  // pixels
    @location(2) uv: vec2<u32>,    // atlas top-left, pixels; for lines, the
                                   // cell width and the line thickness
    @location(3) color: vec4<f32>, // straight alpha
    @location(4) flags: u32,       // 0 solid, 1 font glyph, 2 exact coverage
                                   // mask, 3 curly, 4 dotted, 5 dashed line
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) base: vec2<f32>, // the quad's uv
    @location(2) @interpolate(flat) size: vec2<f32>,
    @location(3) @interpolate(flat) color: vec4<f32>,
    @location(4) @interpolate(flat) flags: u32,
};

@vertex
fn vs_main(q: Quad, @builtin(vertex_index) id: u32) -> VsOut {
    let corner = vec2<f32>(f32(id & 1u), f32(id >> 1u));
    let size = vec2<f32>(q.size);
    let px = vec2<f32>(q.pos) + corner * size;
    var o: VsOut;
    o.pos = vec4<f32>(px * frame.inv_half_size * vec2<f32>(1.0, -1.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    o.uv = vec2<f32>(q.uv) + corner * size;
    o.base = vec2<f32>(q.uv);
    o.size = size;
    o.color = q.color;
    o.flags = q.flags;
    return o;
}

// Coverage of a curly (3), dotted (4) or dashed (5) line at `p` in a box
// of `size`, for cells `cell` pixels wide and lines `t` thick. Each
// pattern repeats a whole number of times per cell, so the lines of
// neighbouring cells join up.
fn line_coverage(kind: u32, p: vec2<f32>, size: vec2<f32>, cell: f32, t: f32) -> f32 {
    if kind == 3u {
        // One wave per cell, from the bottom of the box to the top.
        let amp = (size.y - t) * 0.5;
        let k = 6.2831853 / cell;
        let y = size.y * 0.5 + amp * cos(p.x * k);
        let slope = amp * k * sin(p.x * k);
        // Distance to the curve, near enough, against half the thickness.
        let d = abs(p.y - y) / sqrt(1.0 + slope * slope);
        return saturate(t * 0.5 + 0.5 - d);
    }
    // Dots as long as the line is thick with gaps as long, or dashes
    // four times that with gaps half as long.
    let period = select(6.0 * t, 2.0 * t, kind == 4u);
    let n = max(1.0, floor(cell / period + 0.5));
    let on = select(2.0 / 3.0, 0.5, kind == 4u);
    return select(0.0, 1.0, fract(p.x * n / cell) < on);
}

fn enhance_contrast(alpha: f32, k: f32) -> f32 {
    return alpha * (k + 1.0) / (alpha * k + 1.0);
}

fn apply_alpha_correction(a: f32, f: f32, g: vec4<f32>) -> f32 {
    return a + a * (1.0 - a) * ((g.x * f + g.y) * a + (g.z * f + g.w));
}

@fragment
fn fs_main(i: VsOut) -> @location(0) vec4<f32> {
    var a = i.color.a;
    if i.flags >= 3u {
        a *= line_coverage(i.flags, i.uv - i.base, i.size, i.base.x, i.base.y);
    } else if i.flags != 0u {
        var cov = textureLoad(atlas, vec2<i32>(i.uv), 0).r;
        if i.flags == 1u {
            let c = i.color.rgb;
            // Light text on a dark background gets less contrast boost.
            let k = frame.enhanced_contrast * saturate(dot(c, vec3<f32>(0.30, 0.59, 0.11)) * -4.0 + 3.0);
            cov = enhance_contrast(cov, k);
            cov = apply_alpha_correction(cov, dot(c, vec3<f32>(0.25, 0.5, 0.25)), frame.gamma_ratios);
        }
        a *= saturate(cov);
    }
    return vec4<f32>(i.color.rgb * a, a);
}
