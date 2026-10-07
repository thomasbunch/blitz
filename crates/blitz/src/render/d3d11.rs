//! Direct3D 11 device, render target, pipeline, draw, present and readback.

use windows::Win32::Foundation::{
    CloseHandle, DXGI_STATUS_OCCLUDED, E_FAIL, HANDLE, HMODULE, HWND,
};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_10_0,
    D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
    D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
    D3D11_BIND_VERTEX_BUFFER, D3D11_BLEND_DESC, D3D11_BLEND_INV_SRC_ALPHA, D3D11_BLEND_ONE,
    D3D11_BLEND_OP_ADD, D3D11_BOX, D3D11_BUFFER_DESC, D3D11_CPU_ACCESS_READ,
    D3D11_CPU_ACCESS_WRITE, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_SINGLETHREADED,
    D3D11_INPUT_ELEMENT_DESC, D3D11_INPUT_PER_INSTANCE_DATA, D3D11_MAP_READ,
    D3D11_MAP_WRITE_DISCARD, D3D11_MAPPED_SUBRESOURCE, D3D11_RENDER_TARGET_BLEND_DESC,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_DYNAMIC,
    D3D11_USAGE_STAGING, D3D11_VIEWPORT, D3D11CreateDevice, ID3D11BlendState, ID3D11Buffer,
    ID3D11Device, ID3D11DeviceContext, ID3D11InputLayout, ID3D11PixelShader,
    ID3D11RenderTargetView, ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11VertexShader,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8_UNORM,
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R16G16_SINT, DXGI_FORMAT_R16G16_UINT,
    DXGI_FORMAT_R32_UINT, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET, DXGI_MWA_NO_ALT_ENTER, DXGI_PRESENT,
    DXGI_RGBA, DXGI_SCALING_NONE, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
    DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIAdapter, IDXGIDevice, IDXGIFactory2, IDXGISwapChain2,
};
use windows::Win32::System::Threading::WaitForSingleObjectEx;
use windows::core::{Error, Interface, Result, s};

static VS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/shader.vs.dxbc"));
static PS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/shader.ps.dxbc"));

/// Width and height of the glyph atlas texture.
pub const ATLAS_SIZE: u32 = 2048;

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

/// True for errors after which the device must be dropped and recreated.
pub fn is_device_lost(e: &Error) -> bool {
    e.code() == DXGI_ERROR_DEVICE_REMOVED || e.code() == DXGI_ERROR_DEVICE_RESET
}

pub struct Gpu {
    pub device: ID3D11Device,
    pub ctx: ID3D11DeviceContext,
    /// Running on the WARP software rasterizer.
    pub warp: bool,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    layout: ID3D11InputLayout,
    frame: ID3D11Buffer,
    blend: ID3D11BlendState,
    atlas: ID3D11Texture2D,
    atlas_srv: ID3D11ShaderResourceView,
    quads: Option<ID3D11Buffer>,
    quad_cap: usize,
    /// Grayscale contrast boost and [`gamma_ratios`] for glyphs.
    contrast: f32,
    gamma: [f32; 4],
}

/// A render target that lives only on the GPU, for headless rendering.
pub struct Offscreen {
    pub w: u32,
    pub h: u32,
    pub rtv: ID3D11RenderTargetView,
    tex: ID3D11Texture2D,
    staging: ID3D11Texture2D,
}

impl Gpu {
    /// Creates the device on the GPU, or on WARP when that fails, when
    /// `warp` is set, or when `BLITZ_RENDER=warp`.
    pub fn new(warp: bool) -> Result<Self> {
        let warp =
            warp || std::env::var("BLITZ_RENDER").is_ok_and(|v| v.eq_ignore_ascii_case("warp"));
        if !warp && let Ok(gpu) = Self::with_driver(D3D_DRIVER_TYPE_HARDWARE) {
            return Ok(gpu);
        }
        Self::with_driver(D3D_DRIVER_TYPE_WARP)
    }

    fn with_driver(driver: D3D_DRIVER_TYPE) -> Result<Self> {
        let levels = [
            D3D_FEATURE_LEVEL_11_1,
            D3D_FEATURE_LEVEL_11_0,
            D3D_FEATURE_LEVEL_10_1,
            D3D_FEATURE_LEVEL_10_0,
        ];
        let (mut device, mut ctx) = (None, None);
        // SAFETY: the out-pointers are valid for the call.
        unsafe {
            D3D11CreateDevice(
                None,
                driver,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_SINGLETHREADED,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )?;
        }
        let (Some(device), Some(ctx)) = (device, ctx) else {
            return Err(E_FAIL.into());
        };
        // SAFETY: plain resource creation on a live device; every
        // descriptor outlives its call.
        unsafe {
            let mut vs = None;
            device.CreateVertexShader(VS, None, Some(&mut vs))?;
            let mut ps = None;
            device.CreatePixelShader(PS, None, Some(&mut ps))?;
            let elem = |name, format: DXGI_FORMAT, offset| D3D11_INPUT_ELEMENT_DESC {
                SemanticName: name,
                SemanticIndex: 0,
                Format: format,
                InputSlot: 0,
                AlignedByteOffset: offset,
                InputSlotClass: D3D11_INPUT_PER_INSTANCE_DATA,
                InstanceDataStepRate: 1,
            };
            let elems = [
                elem(s!("POS"), DXGI_FORMAT_R16G16_SINT, 0),
                elem(s!("SIZE"), DXGI_FORMAT_R16G16_UINT, 4),
                elem(s!("UV"), DXGI_FORMAT_R16G16_UINT, 8),
                elem(s!("COLOR"), DXGI_FORMAT_R8G8B8A8_UNORM, 12),
                elem(s!("FLAGS"), DXGI_FORMAT_R32_UINT, 16),
            ];
            let mut layout = None;
            device.CreateInputLayout(&elems, VS, Some(&mut layout))?;

            let mut frame = None;
            let desc = D3D11_BUFFER_DESC {
                ByteWidth: 32,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                ..Default::default()
            };
            device.CreateBuffer(&desc, None, Some(&mut frame))?;

            // Premultiplied alpha.
            let mut bd = D3D11_BLEND_DESC::default();
            bd.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
                BlendEnable: true.into(),
                SrcBlend: D3D11_BLEND_ONE,
                DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOp: D3D11_BLEND_OP_ADD,
                SrcBlendAlpha: D3D11_BLEND_ONE,
                DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOpAlpha: D3D11_BLEND_OP_ADD,
                RenderTargetWriteMask: 0xF,
            };
            let mut blend = None;
            device.CreateBlendState(&bd, Some(&mut blend))?;

            let atlas = texture(
                &device,
                ATLAS_SIZE,
                ATLAS_SIZE,
                DXGI_FORMAT_R8_UNORM,
                D3D11_BIND_SHADER_RESOURCE.0 as u32,
            )?;
            let mut atlas_srv = None;
            device.CreateShaderResourceView(&atlas, None, Some(&mut atlas_srv))?;

            match (vs, ps, layout, frame, blend, atlas_srv) {
                (Some(vs), Some(ps), Some(layout), Some(frame), Some(blend), Some(atlas_srv)) => {
                    Ok(Self {
                        device,
                        ctx,
                        warp: driver == D3D_DRIVER_TYPE_WARP,
                        vs,
                        ps,
                        layout,
                        frame,
                        blend,
                        atlas,
                        atlas_srv,
                        quads: None,
                        quad_cap: 0,
                        contrast: 1.0,
                        gamma: gamma_ratios(1.8),
                    })
                }
                _ => Err(E_FAIL.into()),
            }
        }
    }

    /// Sets the text gamma and grayscale contrast boost used for glyphs.
    pub fn set_text_params(&mut self, gamma: f32, contrast: f32) {
        self.gamma = gamma_ratios(gamma);
        self.contrast = contrast;
    }

    /// Copies `alpha` (`w * h` bytes) into the atlas at (`x`, `y`).
    pub fn upload(&self, x: u32, y: u32, w: u32, h: u32, alpha: &[u8]) {
        if w == 0 || h == 0 || alpha.len() < (w * h) as usize {
            return;
        }
        let area = D3D11_BOX {
            left: x,
            top: y,
            front: 0,
            right: x + w,
            bottom: y + h,
            back: 1,
        };
        // SAFETY: `alpha` holds `h` rows of `w` bytes.
        unsafe {
            self.ctx
                .UpdateSubresource(&self.atlas, 0, Some(&area), alpha.as_ptr().cast(), w, 0);
        }
    }

    /// Clears `rtv` to `clear` (`0xRRGGBB`) and draws `quads` in order.
    pub fn draw(
        &mut self,
        rtv: &ID3D11RenderTargetView,
        w: u32,
        h: u32,
        clear: u32,
        quads: &[Quad],
    ) -> Result<()> {
        let [r, g, b, _] = rgba(clear).map(|c| f32::from(c) / 255.0);
        // SAFETY: every resource bound here is alive and owned by `self`
        // or the caller; mapped memory is written within its size.
        unsafe {
            self.ctx.ClearRenderTargetView(rtv, &[r, g, b, 1.0]);
            if quads.is_empty() || w == 0 || h == 0 {
                return Ok(());
            }
            if self.quad_cap < quads.len() {
                let cap = quads.len().next_power_of_two().max(4096);
                let desc = D3D11_BUFFER_DESC {
                    ByteWidth: (cap * size_of::<Quad>()) as u32,
                    Usage: D3D11_USAGE_DYNAMIC,
                    BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32,
                    CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                    ..Default::default()
                };
                self.quads = None;
                self.device
                    .CreateBuffer(&desc, None, Some(&mut self.quads))?;
                self.quad_cap = cap;
            }
            let Some(buf) = self.quads.clone() else {
                return Err(E_FAIL.into());
            };
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            self.ctx
                .Map(&buf, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut m))?;
            std::ptr::copy_nonoverlapping(quads.as_ptr(), m.pData.cast(), quads.len());
            self.ctx.Unmap(&buf, 0);

            let [g0, g1, g2, g3] = self.gamma;
            let frame = [
                2.0 / w as f32,
                2.0 / h as f32,
                self.contrast,
                0.0,
                g0,
                g1,
                g2,
                g3,
            ];
            self.ctx
                .UpdateSubresource(&self.frame, 0, None, frame.as_ptr().cast(), 0, 0);

            let ctx = &self.ctx;
            ctx.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            ctx.RSSetViewports(Some(&[D3D11_VIEWPORT {
                Width: w as f32,
                Height: h as f32,
                MaxDepth: 1.0,
                ..Default::default()
            }]));
            ctx.IASetInputLayout(&self.layout);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            let stride = size_of::<Quad>() as u32;
            ctx.IASetVertexBuffers(0, 1, Some(&Some(buf)), Some(&stride), Some(&0));
            ctx.VSSetShader(&self.vs, None);
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.frame.clone())]));
            ctx.PSSetShader(&self.ps, None);
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.frame.clone())]));
            ctx.PSSetShaderResources(0, Some(&[Some(self.atlas_srv.clone())]));
            ctx.OMSetBlendState(&self.blend, None, 0xffff_ffff);
            ctx.DrawInstanced(4, quads.len() as u32, 0, 0);
        }
        Ok(())
    }

    pub fn offscreen(&self, w: u32, h: u32) -> Result<Offscreen> {
        let format = DXGI_FORMAT_B8G8R8A8_UNORM;
        let tex = texture(
            &self.device,
            w,
            h,
            format,
            D3D11_BIND_RENDER_TARGET.0 as u32,
        )?;
        let desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            ..tex_desc(w, h, format, 0)
        };
        let mut staging = None;
        let mut rtv = None;
        // SAFETY: plain resource creation on a live device.
        unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&mut staging))?;
            self.device
                .CreateRenderTargetView(&tex, None, Some(&mut rtv))?;
        }
        match (staging, rtv) {
            (Some(staging), Some(rtv)) => Ok(Offscreen {
                w,
                h,
                rtv,
                tex,
                staging,
            }),
            _ => Err(E_FAIL.into()),
        }
    }

    /// Copies an offscreen target to memory: `w * h` BGRA pixels, top row
    /// first.
    pub fn read(&self, t: &Offscreen) -> Result<Vec<u8>> {
        let row = t.w as usize * 4;
        let mut out = vec![0u8; row * t.h as usize];
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: the mapped staging texture holds `h` rows of `RowPitch`
        // bytes, each at least `row` long.
        unsafe {
            self.ctx.CopyResource(&t.staging, &t.tex);
            self.ctx
                .Map(&t.staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
            for (y, dst) in out.chunks_exact_mut(row).enumerate() {
                let src = m.pData.cast::<u8>().add(y * m.RowPitch as usize);
                std::ptr::copy_nonoverlapping(src, dst.as_mut_ptr(), row);
            }
            self.ctx.Unmap(&t.staging, 0);
        }
        Ok(out)
    }
}

fn tex_desc(w: u32, h: u32, format: DXGI_FORMAT, bind: u32) -> D3D11_TEXTURE2D_DESC {
    D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: bind,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    }
}

fn texture(
    device: &ID3D11Device,
    w: u32,
    h: u32,
    format: DXGI_FORMAT,
    bind: u32,
) -> Result<ID3D11Texture2D> {
    let mut tex = None;
    // SAFETY: the descriptor outlives the call.
    unsafe { device.CreateTexture2D(&tex_desc(w, h, format, bind), None, Some(&mut tex))? };
    tex.ok_or_else(|| E_FAIL.into())
}

/// A flip-model swap chain on a window, with a frame-latency waitable.
pub struct Swapchain {
    chain: IDXGISwapChain2,
    waitable: HANDLE,
    rtv: Option<ID3D11RenderTargetView>,
    pub w: u32,
    pub h: u32,
}

const CHAIN_FLAGS: DXGI_SWAP_CHAIN_FLAG = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT;

impl Swapchain {
    pub fn new(gpu: &Gpu, hwnd: HWND, w: u32, h: u32) -> Result<Self> {
        let (w, h) = (w.max(1), h.max(1));
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: w,
            Height: h,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 3,
            Scaling: DXGI_SCALING_NONE,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: CHAIN_FLAGS.0 as u32,
            ..Default::default()
        };
        // SAFETY: COM calls on live objects; `desc` outlives the call.
        unsafe {
            let dxgi: IDXGIDevice = gpu.device.cast()?;
            let adapter: IDXGIAdapter = dxgi.GetAdapter()?;
            let factory: IDXGIFactory2 = adapter.GetParent()?;
            let chain: IDXGISwapChain2 = factory
                .CreateSwapChainForHwnd(&gpu.device, hwnd, &desc, None, None)?
                .cast()?;
            // Alt+Enter belongs to the terminal, not to DXGI fullscreen.
            factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER)?;
            chain.SetMaximumFrameLatency(1)?;
            let waitable = chain.GetFrameLatencyWaitableObject();
            Ok(Self {
                chain,
                waitable,
                rtv: None,
                w,
                h,
            })
        }
    }

    /// The current back buffer's render target view.
    pub fn rtv(&mut self, gpu: &Gpu) -> Result<ID3D11RenderTargetView> {
        if let Some(rtv) = &self.rtv {
            return Ok(rtv.clone());
        }
        let mut rtv = None;
        // SAFETY: COM calls on live objects.
        unsafe {
            let back: ID3D11Texture2D = self.chain.GetBuffer(0)?;
            gpu.device
                .CreateRenderTargetView(&back, None, Some(&mut rtv))?;
        }
        self.rtv = rtv.clone();
        rtv.ok_or_else(|| E_FAIL.into())
    }

    pub fn resize(&mut self, gpu: &Gpu, w: u32, h: u32) -> Result<()> {
        let (w, h) = (w.max(1), h.max(1));
        if (w, h) == (self.w, self.h) {
            return Ok(());
        }
        self.rtv = None;
        // SAFETY: every reference to the back buffers is released first.
        unsafe {
            gpu.ctx.OMSetRenderTargets(None, None);
            gpu.ctx.Flush();
            self.chain
                .ResizeBuffers(0, w, h, DXGI_FORMAT_UNKNOWN, CHAIN_FLAGS)?;
        }
        (self.w, self.h) = (w, h);
        Ok(())
    }

    /// Sets what shows where the window is bigger than the last frame, as
    /// while it is being resized: `bg` rather than black.
    pub fn set_background(&self, bg: u32) {
        let [r, g, b, a] = rgba(bg).map(|c| f32::from(c) / 255.0);
        // SAFETY: COM call on a live swap chain; the colour outlives it.
        let _ = unsafe { self.chain.SetBackgroundColor(&DXGI_RGBA { r, g, b, a }) };
    }

    /// Waits until the swap chain can take another frame, at most `ms`.
    pub fn wait(&self, ms: u32) {
        // SAFETY: `waitable` stays open until drop.
        unsafe {
            WaitForSingleObjectEx(self.waitable, ms, true);
        }
    }

    /// Presents with vsync. Returns false while the window is occluded.
    pub fn present(&self) -> Result<bool> {
        // SAFETY: COM call on a live swap chain.
        let hr = unsafe { self.chain.Present(1, DXGI_PRESENT(0)) };
        if hr == DXGI_STATUS_OCCLUDED {
            return Ok(false);
        }
        hr.ok().map(|()| true)
    }
}

impl Drop for Swapchain {
    fn drop(&mut self) {
        // SAFETY: the handle came from GetFrameLatencyWaitableObject.
        unsafe {
            let _ = CloseHandle(self.waitable);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quad_is_20_bytes() {
        assert_eq!(size_of::<Quad>(), 20);
    }

    #[test]
    fn only_a_removed_or_reset_device_is_lost() {
        assert!(is_device_lost(&Error::from(DXGI_ERROR_DEVICE_REMOVED)));
        assert!(is_device_lost(&Error::from(DXGI_ERROR_DEVICE_RESET)));
        for other in [E_FAIL, DXGI_STATUS_OCCLUDED] {
            assert!(!is_device_lost(&Error::from(other)), "{other:?}");
        }
    }

    /// A live resize shows the theme's background beyond the last frame,
    /// not black strips.
    #[test]
    fn warp_swap_chain_fills_past_the_frame_with_the_background() {
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, WINDOW_EX_STYLE,
            WNDCLASSW, WS_OVERLAPPEDWINDOW,
        };
        use windows::core::w;
        unsafe extern "system" fn wndproc(
            h: HWND,
            m: u32,
            w: windows::Win32::Foundation::WPARAM,
            l: windows::Win32::Foundation::LPARAM,
        ) -> windows::Win32::Foundation::LRESULT {
            // SAFETY: the arguments the system passed.
            unsafe { DefWindowProcW(h, m, w, l) }
        }
        let class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            lpszClassName: w!("blitz.test.swapchain"),
            ..Default::default()
        };
        // SAFETY: a class with a static name; a hidden window of it that is
        // destroyed at the end.
        let hwnd = unsafe {
            RegisterClassW(&class);
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class.lpszClassName,
                w!(""),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                64,
                64,
                None,
                None,
                None,
                None,
            )
        }
        .expect("window");
        let gpu = Gpu::new(true).expect("WARP device");
        let chain = Swapchain::new(&gpu, hwnd, 64, 64).expect("swap chain");
        chain.set_background(0x336699);
        // SAFETY: COM call on a live swap chain.
        let bg = unsafe { chain.chain.GetBackgroundColor() }.expect("colour");
        drop(chain);
        // SAFETY: the window made above.
        let _ = unsafe { DestroyWindow(hwnd) };
        let byte = |c: f32| (c * 255.0).round() as u32;
        assert_eq!((byte(bg.r), byte(bg.g), byte(bg.b)), (0x33, 0x66, 0x99));
        assert_eq!(bg.a, 1.0);
    }

    #[test]
    fn gamma_ratios_match_directwrite_defaults() {
        let close = |a: [f32; 4], b: [f32; 4]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6);
        // The values DirectWrite uses at its default gamma of 1.8.
        let default = [0.148_054_42, -0.894_594_55, 1.475_908, -0.324_668_26];
        assert!(close(gamma_ratios(1.8), default));
        assert!(close(gamma_ratios(1.84), default));
        assert_eq!(gamma_ratios(0.5), [0.0; 4]);
        assert_eq!(gamma_ratios(9.0), gamma_ratios(2.2));
    }

    #[test]
    fn warp_draws_solid_and_masked_quads_offscreen() {
        let mut gpu = Gpu::new(true).expect("WARP device");
        assert!(gpu.warp);
        let target = gpu.offscreen(16, 8).expect("target");
        // A 4x4 coverage mask: left half full, right half empty.
        let mask: Vec<u8> = (0..16).map(|i| if i % 4 < 2 { 255 } else { 0 }).collect();
        gpu.upload(0, 0, 4, 4, &mask);
        let quads = [
            Quad {
                pos: [0, 0],
                size: [8, 8],
                color: rgba(0xff0000),
                ..Default::default()
            },
            Quad {
                pos: [8, 0],
                size: [4, 4],
                uv: [0, 0],
                color: rgba(0x00ff00),
                flags: MASK,
            },
        ];
        gpu.draw(&target.rtv, 16, 8, 0x000080, &quads)
            .expect("draw");
        let px = gpu.read(&target).expect("read");
        let at = |x: usize, y: usize| {
            let i = (y * 16 + x) * 4;
            [px[i], px[i + 1], px[i + 2]]
        };
        // BGRA bytes.
        assert_eq!(at(0, 0), [0, 0, 255]);
        assert_eq!(at(7, 7), [0, 0, 255]);
        assert_eq!(at(8, 0), [0, 255, 0]);
        assert_eq!(at(9, 3), [0, 255, 0]);
        assert_eq!(at(10, 0), [128, 0, 0]);
        assert_eq!(at(15, 7), [128, 0, 0]);
    }

    /// A hidden 64x48 popup window. Desktop composition, which flip-model
    /// swap chains need, is always on since Windows 8, so these tests run
    /// everywhere blitz does, CI included, and fail rather than skip.
    fn hidden_window() -> HWND {
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, WINDOW_EX_STYLE, WS_POPUP};
        use windows::core::w;

        // SAFETY: a plain hidden top-level window of a system class.
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!(""),
                WS_POPUP,
                0,
                0,
                64,
                48,
                None,
                None,
                None,
                None,
            )
        };
        hwnd.expect("window")
    }

    fn destroy(hwnd: HWND) {
        // SAFETY: the window was created on this thread.
        unsafe { windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd) }.expect("destroy");
    }

    fn present_one(gpu: &mut Gpu, chain: &mut Swapchain) {
        chain.wait(100);
        let rtv = chain.rtv(gpu).expect("back buffer");
        let quad = Quad {
            size: [8, 8],
            color: rgba(0xffffff),
            ..Default::default()
        };
        gpu.draw(&rtv, chain.w, chain.h, 0x101010, &[quad])
            .expect("draw");
        chain.present().expect("present");
    }

    #[test]
    fn swapchain_on_a_hidden_window_presents_and_resizes() {
        let hwnd = hidden_window();
        let mut gpu = Gpu::new(true).expect("WARP device");
        let mut chain = Swapchain::new(&gpu, hwnd, 64, 48).expect("swap chain");
        for (w, h) in [(64, 48), (32, 20), (0, 0)] {
            chain.resize(&gpu, w, h).expect("resize");
            present_one(&mut gpu, &mut chain);
        }
        assert_eq!((chain.w, chain.h), (1, 1));
        drop(chain);
        destroy(hwnd);
    }

    /// What the app does after a lost device: drop the device and swap
    /// chain, then build both again on the same window.
    #[test]
    fn swapchain_can_be_rebuilt_on_the_same_window() {
        let hwnd = hidden_window();
        for _ in 0..3 {
            let mut gpu = Gpu::new(true).expect("WARP device");
            let mut chain = Swapchain::new(&gpu, hwnd, 64, 48).expect("swap chain");
            present_one(&mut gpu, &mut chain);
        }
        // A new swap chain on a device whose context still has the old
        // back buffer bound.
        let mut gpu = Gpu::new(true).expect("WARP device");
        for _ in 0..3 {
            let mut chain = Swapchain::new(&gpu, hwnd, 64, 48).expect("swap chain again");
            present_one(&mut gpu, &mut chain);
        }
        destroy(hwnd);
    }
}
