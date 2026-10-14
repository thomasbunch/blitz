//! The instanced rectangles every renderer backend draws, and the
//! constants its shader shares with them.

/// Width and height of the glyph atlas texture: room for a 4K screen of
/// distinct wide glyphs, so a full one never clears it every frame.
/// Direct3D feature level 10_0 and wgpu's default limits allow 8192.
pub const ATLAS_SIZE: u32 = 4096;

/// The four alpha-correction constants the shader needs for a text
/// `gamma`, as DirectWrite computes them for grayscale text.
///
/// Adapted from Windows Terminal's dwrite_helpers.cpp (DWrite_GetGammaRatios):
/// Copyright (c) Microsoft Corporation. Licensed under the MIT License.
pub fn gamma_ratios(gamma: f32) -> [f32; 4] {
    // One row per gamma from 1.0 to 2.2 in steps of 0.1.
    const RATIOS: [[f32; 4]; 13] = [
        [0.0000, 0.0000, 0.0000, 0.0000],
        [0.0166, -0.0807, 0.2227, -0.0751],
        [0.0350, -0.1760, 0.4325, -0.1370],
        [0.0543, -0.2821, 0.6302, -0.1876],
        [0.0739, -0.3963, 0.8167, -0.2287],
        [0.0933, -0.5161, 0.9926, -0.2616],
        [0.1121, -0.6395, 1.1588, -0.2877],
        [0.1300, -0.7649, 1.3159, -0.3080],
        [0.1469, -0.8911, 1.4644, -0.3234],
        [0.1627, -1.0170, 1.6051, -0.3347],
        [0.1773, -1.1420, 1.7385, -0.3426],
        [0.1908, -1.2652, 1.8650, -0.3476],
        [0.2031, -1.3864, 1.9851, -0.3501],
    ];
    let norm13 = (f64::from(0x10000) / (255.0 * 255.0)) as f32;
    let norm24 = (f64::from(0x100) / 255.0) as f32;
    let i = ((gamma * 10.0 + 0.5) as i32).clamp(10, 22) as usize - 10;
    let [a, b, c, d] = RATIOS[i];
    [norm13 * a, norm24 * b, norm13 * c, norm24 * d]
}

/// [`Quad::flags`]: a solid rectangle.
pub const SOLID: u32 = 0;
/// A font glyph: atlas coverage with contrast and gamma correction.
pub const GLYPH: u32 = 1;
/// A procedural glyph: atlas coverage used as is.
pub const MASK: u32 = 2;
/// Curly, dotted and dashed lines, drawn by the shader. [`Quad::uv`] holds
/// the cell width and the line thickness.
pub const CURLY: u32 = 3;
pub const DOTTED: u32 = 4;
pub const DASHED: u32 = 5;

/// One instanced rectangle, 20 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Quad {
    pub pos: [i16; 2],
    pub size: [u16; 2],
    /// Atlas position of the top-left corner; unused for [`SOLID`], and
    /// line sizes for [`CURLY`] and the other lines.
    pub uv: [u16; 2],
    /// R, G, B, A with straight alpha.
    pub color: [u8; 4],
    pub flags: u32,
}

/// `0xRRGGBB` to opaque [`Quad::color`].
pub fn rgba(rgb: u32) -> [u8; 4] {
    let [_, r, g, b] = rgb.to_be_bytes();
    [r, g, b, 255]
}
