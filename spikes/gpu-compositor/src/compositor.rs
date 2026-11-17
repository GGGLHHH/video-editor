//! 显卡合成器：每层一张纹理，按矩形 / 裁剪 / 不透明度画到输出纹理上，再读回内存（BGRA）。

use bytemuck::{Pod, Zeroable};
use std::time::Instant;

const SHADER: &str = r"
struct Layer {
    rect: vec4<f32>,   // 输出画面上的位置（NDC）：x0, y0(上), x1, y1(下)
    crop: vec4<f32>,   // 纹理坐标：u0, v0, u1, v1
    opacity: vec4<f32>,
};
@group(0) @binding(0) var<uniform> layer: Layer;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VOut {
    var corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
    );
    let c = corners[i];
    var o: VOut;
    o.pos = vec4(mix(layer.rect.x, layer.rect.z, c.x), mix(layer.rect.y, layer.rect.w, c.y), 0.0, 1.0);
    o.uv = vec2(mix(layer.crop.x, layer.crop.z, c.x), mix(layer.crop.y, layer.crop.w, c.y));
    return o;
}

@fragment
fn fs(v: VOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, v.uv);
    return vec4(c.rgb, c.a * layer.opacity.x);
}
";

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LayerUniform {
    rect: [f32; 4],
    crop: [f32; 4],
    opacity: [f32; 4],
}

/// 一层的放置方式（像素坐标，输出画面左上角为原点）。
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub opacity: f32,
}

struct LayerSlot {
    texture: wgpu::Texture,
    size: (u32, u32),
    uniform: wgpu::Buffer,
    bind: wgpu::BindGroup,
}

/// 每帧耗时（毫秒）。
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameTiming {
    pub upload_ms: f64,
    pub render_readback_ms: f64,
}

pub struct Compositor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    layers: Vec<LayerSlot>,
    output: wgpu::Texture,
    readback: wgpu::Buffer,
    out_size: (u32, u32),
    padded_row: u32,
    pub adapter_info: String,
}

impl Compositor {
    /// `layer_sizes`：每层纹理尺寸；`out`：输出画面尺寸。
    pub fn new(layer_sizes: &[(u32, u32)], out: (u32, u32)) -> Self {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: std::env::var_os("FALLBACK").is_some(),
            ..Default::default()
        }))
        .expect("没有可用的显卡");
        let info = adapter.get_info();
        let adapter_info = format!("{} ({:?})", info.name, info.backend);
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("创建设备失败");

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("compose"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
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
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&bgl)],
            ..Default::default()
        });
        let format = wgpu::TextureFormat::Bgra8Unorm;
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("compose"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let layers = layer_sizes
            .iter()
            .map(|&(w, h)| {
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: std::mem::size_of::<LayerUniform>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &bgl,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
                    ],
                });
                LayerSlot { texture, size: (w, h), uniform, bind }
            })
            .collect();

        let output = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("output"),
            size: wgpu::Extent3d { width: out.0, height: out.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let padded_row = (out.0 * 4).div_ceil(256) * 256;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(padded_row) * u64::from(out.1),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { device, queue, pipeline, layers, output, readback, out_size: out, padded_row, adapter_info }
    }

    /// 合成一帧：上传各层像素（BGRA），按放置方式画，读回到 `out`（紧密 BGRA）。
    pub fn compose(&mut self, pixels: &[&[u8]], places: &[Placement], out: &mut Vec<u8>) -> FrameTiming {
        let t0 = Instant::now();
        let (ow, oh) = (self.out_size.0 as f32, self.out_size.1 as f32);
        for ((slot, data), p) in self.layers.iter().zip(pixels).zip(places) {
            let (w, h) = slot.size;
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &slot.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            let u = LayerUniform {
                rect: [p.x / ow * 2.0 - 1.0, 1.0 - p.y / oh * 2.0, (p.x + p.w) / ow * 2.0 - 1.0, 1.0 - (p.y + p.h) / oh * 2.0],
                crop: [0.0, 0.0, 1.0, 1.0],
                opacity: [p.opacity, 0.0, 0.0, 0.0],
            };
            self.queue.write_buffer(&slot.uniform, 0, bytemuck::bytes_of(&u));
        }
        let upload_ms = t0.elapsed().as_secs_f64() * 1000.0;

        let t1 = Instant::now();
        let view = self.output.create_view(&wgpu::TextureViewDescriptor::default());
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            for slot in &self.layers {
                pass.set_bind_group(0, &slot.bind, &[]);
                pass.draw(0..6, 0..1);
            }
        }
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.output,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_row),
                    rows_per_image: Some(self.out_size.1),
                },
            },
            wgpu::Extent3d { width: self.out_size.0, height: self.out_size.1, depth_or_array_layers: 1 },
        );
        self.queue.submit([enc.finish()]);
        let slice = self.readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("读回映射失败"));
        self.device.poll(wgpu::PollType::wait_indefinitely()).expect("等待显卡失败");
        {
            let mapped = slice.get_mapped_range().expect("读回映射");
            let row = (self.out_size.0 * 4) as usize;
            out.clear();
            out.reserve(row * self.out_size.1 as usize);
            for y in 0..self.out_size.1 as usize {
                let start = y * self.padded_row as usize;
                out.extend_from_slice(&mapped[start..start + row]);
            }
        }
        self.readback.unmap();
        FrameTiming { upload_ms, render_readback_ms: t1.elapsed().as_secs_f64() * 1000.0 }
    }
}

/// 生成测试层：渐变背景、两种色块画面、半透明圆形台标（BGRA）。
pub fn make_layer(kind: u32, w: u32, h: u32) -> Vec<u8> {
    let mut v = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let (b, g, r, a) = match kind {
                0 => ((x * 255 / w) as u8, (y * 255 / h) as u8, 80, 255),
                1 => (40, 160, ((x / 64 + y / 64) % 2 * 200) as u8, 255),
                2 => (200, ((x / 32) % 2 * 180) as u8, 60, 255),
                _ => {
                    let (dx, dy) = (x as f32 - w as f32 / 2.0, y as f32 - h as f32 / 2.0);
                    let inside = dx * dx + dy * dy < (w as f32 / 2.0).powi(2);
                    if inside { (255, 255, 255, 160) } else { (0, 0, 0, 0) }
                }
            };
            v[i..i + 4].copy_from_slice(&[b, g, r, a]);
        }
    }
    v
}

/// 第 `n` 帧的放置：背景铺满，两个画中画在动，台标在右上角。
pub fn placements(n: u64, out: (u32, u32)) -> Vec<Placement> {
    let (w, h) = (out.0 as f32, out.1 as f32);
    let t = n as f32 / 60.0;
    vec![
        Placement { x: 0.0, y: 0.0, w, h, opacity: 1.0 },
        Placement { x: w * (0.05 + 0.3 * (t * 0.7).sin().abs()), y: h * 0.55, w: w / 3.0, h: h / 3.0, opacity: 1.0 },
        Placement { x: w * 0.6, y: h * (0.05 + 0.3 * (t * 0.5).cos().abs()), w: w / 3.0, h: h / 3.0, opacity: 0.85 },
        Placement { x: w * 0.88, y: h * 0.04, w: w * 0.08, h: w * 0.08, opacity: 1.0 },
    ]
}
