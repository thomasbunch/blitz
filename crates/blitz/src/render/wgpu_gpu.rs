//! wgpu device, pipeline, draw and readback, for macOS and Linux: what
//! d3d11.rs does on Windows, with the same shader in WGSL.

use super::Result;
pub use super::quad::*;

/// What a frame is drawn to.
pub type Target = wgpu::TextureView;

/// Offscreen targets and readback use this; a window's surface may use
/// another 8-bit unorm format, never an sRGB one, so blending happens on
/// the colours as given, as on Windows.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// Running on a software rasterizer, as WARP is on Windows.
    pub warp: bool,
    /// The adapter's name and backend, for reports.
    pub name: String,
    pipelines: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    layout: wgpu::PipelineLayout,
    shader: wgpu::ShaderModule,
    frame: wgpu::Buffer,
    bind: wgpu::BindGroup,
    atlas: wgpu::Texture,
    quads: Option<wgpu::Buffer>,
    quad_cap: usize,
    /// Grayscale contrast boost and [`gamma_ratios`] for glyphs.
    contrast: f32,
    gamma: [f32; 4],
}

/// A render target that lives only on the GPU, for headless rendering.
pub struct Offscreen {
    pub w: u32,
    pub h: u32,
    pub rtv: wgpu::TextureView,
    tex: wgpu::Texture,
}

fn instance() -> wgpu::Instance {
    wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY | wgpu::Backends::GL,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    })
}

impl Gpu {
    /// Creates the device on the GPU, or on a software rasterizer when
    /// that fails, when `warp` is set, or when `BLITZ_RENDER=warp`.
    pub fn new(warp: bool) -> Result<Self> {
        let warp =
            warp || std::env::var("BLITZ_RENDER").is_ok_and(|v| v.eq_ignore_ascii_case("warp"));
        let instance = instance();
        let pick = |fallback| {
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                force_fallback_adapter: fallback,
                compatible_surface: None,
                ..Default::default()
            }))
        };
        let adapter = match (warp, pick(warp)) {
            (_, Ok(a)) => a,
            // A Mac has no software rasterizer; its GPU stands in.
            (fallback, Err(_)) => pick(!fallback)?,
        };
        Self::with_adapter(&adapter)
    }

    fn with_adapter(adapter: &wgpu::Adapter) -> Result<Self> {
        let info = adapter.get_info();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("blitz"),
                required_limits:
                    wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
                ..Default::default()
            }))?;
        let shader = device.create_shader_module(wgpu::include_wgsl!("shader.wgsl"));
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("quads"),
            bind_group_layouts: &[Some(&bind_layout)],
            ..Default::default()
        });
        let frame = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = atlas.create_view(&Default::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame"),
            layout: &bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: frame.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        Ok(Self {
            device,
            queue,
            warp: info.device_type == wgpu::DeviceType::Cpu,
            name: format!("{} ({:?})", info.name, info.backend),
            pipelines: Vec::new(),
            layout,
            shader,
            frame,
            bind,
            atlas,
            quads: None,
            quad_cap: 0,
            contrast: 1.0,
            gamma: gamma_ratios(1.8),
        })
    }

    /// The pipeline that draws to targets of `format`, made at first use.
    fn pipeline(&mut self, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
        if let Some((_, p)) = self.pipelines.iter().find(|(f, _)| *f == format) {
            return p.clone();
        }
        let attrs = wgpu::vertex_attr_array![
            0 => Sint16x2,
            1 => Uint16x2,
            2 => Uint16x2,
            3 => Unorm8x4,
            4 => Uint32,
        ];
        let p = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("quads"),
                layout: Some(&self.layout),
                vertex: wgpu::VertexState {
                    module: &self.shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<Quad>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &attrs,
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &self.shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });
        self.pipelines.push((format, p.clone()));
        p
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
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.atlas,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &alpha[..(w * h) as usize],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Clears `rtv` to `clear` (`0xRRGGBB`) and draws `quads` in order.
    pub fn draw(
        &mut self,
        rtv: &wgpu::TextureView,
        w: u32,
        h: u32,
        clear: u32,
        quads: &[Quad],
    ) -> Result<()> {
        let [r, g, b, _] = rgba(clear).map(|c| f64::from(c) / 255.0);
        let format = rtv.texture().format();
        let pipeline = (!quads.is_empty() && w > 0 && h > 0).then(|| self.pipeline(format));
        if pipeline.is_some() {
            if self.quad_cap < quads.len() {
                let cap = quads.len().next_power_of_two().max(4096);
                self.quads = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("quads"),
                    size: (cap * size_of::<Quad>()) as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
                self.quad_cap = cap;
            }
            if let Some(buf) = &self.quads {
                self.queue.write_buffer(buf, 0, quad_bytes(quads));
            }
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
            let bytes: Vec<u8> = frame.iter().flat_map(|f| f.to_le_bytes()).collect();
            self.queue.write_buffer(&self.frame, 0, &bytes);
        }
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: rtv,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            if let (Some(p), Some(buf)) = (&pipeline, &self.quads) {
                pass.set_pipeline(p);
                pass.set_bind_group(0, &self.bind, &[]);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.set_viewport(0.0, 0.0, w as f32, h as f32, 0.0, 1.0);
                pass.draw(0..4, 0..quads.len() as u32);
            }
        }
        self.queue.submit([enc.finish()]);
        Ok(())
    }

    pub fn offscreen(&self, w: u32, h: u32) -> Result<Offscreen> {
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let rtv = tex.create_view(&Default::default());
        Ok(Offscreen { w, h, rtv, tex })
    }

    /// Copies an offscreen target to memory: `w * h` BGRA pixels, top row
    /// first.
    pub fn read(&self, t: &Offscreen) -> Result<Vec<u8>> {
        let row = t.w as usize * 4;
        // Rows are copied out 256-byte aligned.
        let pitch = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize);
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (pitch * t.h as usize) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            t.tex.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(pitch as u32),
                    rows_per_image: Some(t.h),
                },
            },
            t.tex.size(),
        );
        self.queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        rx.recv()??;
        let data = slice.get_mapped_range()?;
        let mut out = vec![0u8; row * t.h as usize];
        for (dst, src) in out.chunks_exact_mut(row).zip(data.chunks(pitch)) {
            dst.copy_from_slice(&src[..row]);
        }
        drop(data);
        buf.unmap();
        Ok(out)
    }
}

/// `quads` as the bytes the vertex buffer takes.
fn quad_bytes(quads: &[Quad]) -> &[u8] {
    // SAFETY: Quad is repr(C), 20 bytes of plain integers with no padding,
    // so any of its bytes may be read.
    unsafe { std::slice::from_raw_parts(quads.as_ptr().cast(), std::mem::size_of_val(quads)) }
}
