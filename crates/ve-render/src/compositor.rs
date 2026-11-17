//! 合成器。

use crate::RenderError;
use bytemuck::{Pod, Zeroable};
use ve_core::LayerGeometry;

/// 纹理和输出的像素格式。BGRA 和 GPUI、解码器的格式一致，不用换通道。
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

/// 合成器配置。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderConfig {
    /// 强制用软件显卡（WARP）。测试用：结果稳定，不依赖机器上的显卡。
    pub software: bool,
}

/// 一层的像素。
#[derive(Clone, Copy, Debug)]
pub enum FramePixels<'a> {
    /// 紧密排列的 BGRA，预乘透明。
    Bgra {
        /// 宽。
        width: u32,
        /// 高。
        height: u32,
        /// `width × height × 4` 字节。
        data: &'a [u8],
    },
}

impl FramePixels<'_> {
    const fn size(&self) -> (u32, u32) {
        match self {
            Self::Bgra { width, height, .. } => (*width, *height),
        }
    }

    fn check(&self) -> Result<(), RenderError> {
        match self {
            Self::Bgra {
                width,
                height,
                data,
            } => {
                let want = u64::from(*width) * u64::from(*height) * 4;
                if *width == 0 || *height == 0 || data.len() as u64 != want {
                    return Err(RenderError::BadLayer(format!(
                        "{width}×{height} 需要 {want} 字节，实际 {}",
                        data.len()
                    )));
                }
                Ok(())
            }
        }
    }
}

/// 一个图层：像素、在画布上的几何（[`ve_core::layer_geometry`] 算出）、不透明度。
#[derive(Clone, Copy, Debug)]
pub struct Layer<'a> {
    /// 像素。
    pub pixels: FramePixels<'a>,
    /// 几何（画布像素坐标）。
    pub geometry: LayerGeometry,
    /// 不透明度（0–1）。
    pub opacity: f32,
}

/// 每层的参数，作为实例顶点数据传给着色器。
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LayerInstance {
    rect: [f32; 4],
    uv: [f32; 4],
    opacity: f32,
}

const INSTANCE_ATTRS: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32];

/// 一个图层槽位：纹理按尺寸复用。
struct Slot {
    texture: wgpu::Texture,
    size: (u32, u32),
    bind: wgpu::BindGroup,
}

/// 导出用的 NV12：Y 平面（宽×高）后接 UV 交错平面（宽×高/2），紧密排列，宽高为偶数。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Nv12 {
    /// 宽。
    pub width: u32,
    /// 高。
    pub height: u32,
    /// Y 平面 + UV 平面。
    pub data: Vec<u8>,
}

/// NV12 转换用的两个目标纹理和读回缓冲，跟着输出目标一起重建。
struct Nv12Target {
    size: (u32, u32),
    y: wgpu::Texture,
    uv: wgpu::Texture,
    bind: wgpu::BindGroup,
}

/// 一帧 NV12 的读回缓冲（Y、UV 各一个）。在途的每帧各占一套，用完放回池里复用。
struct Readback {
    size: (u32, u32),
    y_buf: wgpu::Buffer,
    uv_buf: wgpu::Buffer,
    row: u32,
}

/// 已经读回、还映射着的一帧 NV12。可以交给别的线程直接写出去（省一次拷贝），
/// 写完用 [`Compositor::recycle`] 还给合成器复用缓冲。
pub struct MappedNv12 {
    readback: Readback,
}

impl std::fmt::Debug for MappedNv12 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MappedNv12")
            .field("size", &self.readback.size)
            .finish_non_exhaustive()
    }
}

impl MappedNv12 {
    /// 画面尺寸。
    #[must_use]
    pub const fn size(&self) -> (u32, u32) {
        self.readback.size
    }

    /// 按紧密排列的 NV12（Y 平面后接 UV 平面）写出。
    ///
    /// # Errors
    /// 写失败，或缓冲映射已失效。
    pub fn write_to(&self, sink: &mut dyn std::io::Write) -> std::io::Result<()> {
        let rb = &self.readback;
        let (w, h) = rb.size;
        for (buf, rows) in [(&rb.y_buf, h), (&rb.uv_buf, h / 2)] {
            let mapped = buf
                .slice(..)
                .get_mapped_range()
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            for chunk in mapped.chunks(rb.row as usize).take(rows as usize) {
                sink.write_all(&chunk[..w as usize])?;
            }
        }
        Ok(())
    }
}

/// 已提交、还没取回的一帧。
struct InFlight {
    readback: Readback,
    submission: wgpu::SubmissionIndex,
    mapped: std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
}

/// NV12 转换的着色器资源。
struct Nv12Pipelines {
    layout: wgpu::BindGroupLayout,
    y: wgpu::RenderPipeline,
    uv: wgpu::RenderPipeline,
}

/// 输出目标和读回缓冲，按输出尺寸复用。
struct Target {
    size: (u32, u32),
    texture: wgpu::Texture,
    readback: wgpu::Buffer,
    padded_row: u32,
}

/// 显卡合成器。
pub struct Compositor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    slots: Vec<Slot>,
    /// 各层参数（实例顶点缓冲）和容量，不够时重建。
    instances: Option<(wgpu::Buffer, usize)>,
    target: Option<Target>,
    nv12: Option<Nv12Pipelines>,
    nv12_target: Option<Nv12Target>,
    readbacks: Vec<Readback>,
    in_flight: std::collections::VecDeque<InFlight>,
    adapter: String,
    software: bool,
}

impl std::fmt::Debug for Compositor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Compositor")
            .field("adapter", &self.adapter)
            .field("slots", &self.slots.len())
            .finish_non_exhaustive()
    }
}

/// 软件显卡（WARP）多个设备同时工作会让进程崩溃（访问违例），实测并行跑测试时必现。
/// 软件模式下，创建和每次合成都串行化；硬件显卡不受影响。
static SOFTWARE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn software_guard(software: bool) -> Option<std::sync::MutexGuard<'static, ()>> {
    software.then(|| {
        SOFTWARE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    })
}

type DeviceParts = (wgpu::Device, wgpu::Queue, String, bool);

/// 选显卡并创建设备；返回设备、队列、显卡名、是不是软件显卡。
fn create_device(software: bool) -> Result<DeviceParts, RenderError> {
    let instance = instance();
    let adapter = adapter(&instance, software).ok_or(RenderError::NoAdapter)?;
    let info = adapter.get_info();
    let name = format!("{} ({:?})", info.name, info.backend);
    // 按显卡实际支持的上限创建设备（默认限制里纹理最大 8192，大图会超）
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("ve-render"),
        required_limits: adapter.limits(),
        ..Default::default()
    }))
    .map_err(|e| RenderError::Device(e.to_string()))?;
    Ok((
        device,
        queue,
        name,
        info.device_type == wgpu::DeviceType::Cpu,
    ))
}

/// 软件模式下所有合成器共用一个 WARP 设备：每次新建要几百毫秒，测试里几十个合成器排队建会超时。
fn shared_software_device() -> Result<DeviceParts, RenderError> {
    static DEVICE: std::sync::OnceLock<Result<DeviceParts, String>> = std::sync::OnceLock::new();
    DEVICE
        .get_or_init(|| create_device(true).map_err(|e| e.to_string()))
        .clone()
        .map_err(RenderError::Device)
}

fn instance() -> wgpu::Instance {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    // Windows 上用 DX12：实测比 Vulkan 略快，系统自带，而且有 WARP 软件显卡可用于测试
    if cfg!(windows) {
        desc.backends = wgpu::Backends::DX12;
    }
    wgpu::Instance::new(desc)
}

fn adapter(instance: &wgpu::Instance, software: bool) -> Option<wgpu::Adapter> {
    let request = |fallback: bool| {
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: fallback,
            ..Default::default()
        }))
        .ok()
    };
    if software {
        return request(true);
    }
    request(false).or_else(|| {
        log::warn!("没有可用的硬件显卡，合成改用软件显卡（会很慢）");
        request(true)
    })
}

fn layer_bind_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("layer"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

fn compose_pipeline(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("compose"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("compose"),
        bind_group_layouts: &[Some(layout)],
        ..Default::default()
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("compose"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: size_of::<LayerInstance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &INSTANCE_ATTRS,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            targets: &[Some(wgpu::ColorTargetState {
                format: FORMAT,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

impl Compositor {
    /// 创建合成器（选显卡、编译着色器）。
    ///
    /// # Errors
    /// 找不到显卡，或创建设备失败。
    pub fn new(config: &RenderConfig) -> Result<Self, RenderError> {
        let _serial = software_guard(config.software);
        let (device, queue, name, cpu) = if config.software {
            shared_software_device()?
        } else {
            match create_device(false)? {
                // 没有硬件显卡、退回了软件显卡：和软件模式一样共用一个设备（多个 WARP 设备并行会崩）
                parts if parts.3 => shared_software_device()?,
                parts => parts,
            }
        };
        let layout = layer_bind_layout(&device);
        let pipeline = compose_pipeline(&device, &layout);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("layer"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Ok(Self {
            device,
            queue,
            pipeline,
            layout,
            sampler,
            slots: Vec::new(),
            instances: None,
            target: None,
            nv12: None,
            nv12_target: None,
            readbacks: Vec::new(),
            in_flight: std::collections::VecDeque::new(),
            adapter: name,
            software: cpu || config.software,
        })
    }

    /// 用的是哪块显卡。
    #[must_use]
    pub fn adapter_name(&self) -> &str {
        &self.adapter
    }

    /// 图层和输出纹理的最大边长（像素）。超过的图层要先缩小再交进来。
    #[must_use]
    pub fn max_texture_size(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    fn new_slot(&self, size: (u32, u32)) -> Slot {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("layer"),
            size: extent(size),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("layer"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        Slot {
            texture,
            size,
            bind,
        }
    }

    fn ensure_target(&mut self, size: (u32, u32)) {
        if self.target.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("output"),
            size: extent(size),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let padded_row = padded(size.0 * 4);
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(padded_row) * u64::from(size.1),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.target = Some(Target {
            size,
            texture,
            readback,
            padded_row,
        });
        self.nv12_target = None;
    }

    /// 上传各层像素和参数（槽位按尺寸复用）。
    fn upload(&mut self, size: (u32, u32), layers: &[Layer<'_>]) -> Result<(), RenderError> {
        let max = self.max_texture_size();
        if size.0 == 0 || size.1 == 0 || size.0 > max || size.1 > max {
            return Err(RenderError::BadLayer(format!(
                "输出尺寸 {}×{} 无效（最大边长 {max}）",
                size.0, size.1
            )));
        }
        for layer in layers {
            layer.pixels.check()?;
            let (w, h) = layer.pixels.size();
            if w > max || h > max {
                return Err(RenderError::BadLayer(format!(
                    "图层 {w}×{h} 超过显卡支持的最大边长 {max}"
                )));
            }
        }
        let (cw, ch) = (f64::from(size.0), f64::from(size.1));
        let mut instances = Vec::with_capacity(layers.len());
        for (i, layer) in layers.iter().enumerate() {
            let pix = layer.pixels.size();
            if self.slots.get(i).is_none_or(|s| s.size != pix) {
                let slot = self.new_slot(pix);
                if i < self.slots.len() {
                    self.slots[i] = slot;
                } else {
                    self.slots.push(slot);
                }
            }
            let slot = &self.slots[i];
            let FramePixels::Bgra { data, .. } = layer.pixels;
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &slot.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(pix.0 * 4),
                    rows_per_image: Some(pix.1),
                },
                extent(pix),
            );
            let shown = layer.geometry.shown;
            let uv = layer.geometry.uv;
            let ndc_x = |x: f32| to_f32(f64::from(x) / cw * 2.0 - 1.0);
            let ndc_y = |y: f32| to_f32(1.0 - f64::from(y) / ch * 2.0);
            instances.push(LayerInstance {
                rect: [
                    ndc_x(shown.x),
                    ndc_y(shown.y),
                    ndc_x(shown.x + shown.w),
                    ndc_y(shown.y + shown.h),
                ],
                uv: [uv.x, uv.y, uv.x + uv.w, uv.y + uv.h],
                opacity: layer.opacity.clamp(0.0, 1.0),
            });
        }
        self.write_instances(&instances);
        Ok(())
    }

    /// 写各层参数；容量不够时按 2 的幂重建缓冲。
    fn write_instances(&mut self, instances: &[LayerInstance]) {
        if instances.is_empty() {
            return;
        }
        if self
            .instances
            .as_ref()
            .is_none_or(|(_, cap)| *cap < instances.len())
        {
            let cap = instances.len().next_power_of_two();
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("layers"),
                size: (cap * size_of::<LayerInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instances = Some((buffer, cap));
        }
        if let Some((buffer, _)) = &self.instances {
            self.queue
                .write_buffer(buffer, 0, bytemuck::cast_slice(instances));
        }
    }

    /// 把各层画到输出纹理上（黑底）。
    fn draw(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, count: usize) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("compose"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        let Some((instances, _)) = &self.instances else {
            return;
        };
        pass.set_vertex_buffer(0, instances.slice(..));
        for (i, slot) in (0u32..).zip(&self.slots[..count]) {
            pass.set_bind_group(0, &slot.bind, &[]);
            pass.draw(0..6, i..i + 1);
        }
    }

    /// 上传并画到输出纹理，返回编码器（还没提交）。
    fn render(
        &mut self,
        size: (u32, u32),
        layers: &[Layer<'_>],
    ) -> Result<wgpu::CommandEncoder, RenderError> {
        self.upload(size, layers)?;
        self.ensure_target(size);
        let target = self
            .target
            .as_ref()
            .ok_or_else(|| RenderError::Device("输出目标没建好".into()))?;
        let view = target
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        self.draw(&mut encoder, &view, layers.len());
        Ok(encoder)
    }

    /// 合成一帧，读回成紧密排列的 BGRA（给预览用）。
    ///
    /// # Errors
    /// 输出尺寸为零、图层数据和尺寸对不上、读回失败。
    pub fn compose_bgra(
        &mut self,
        size: (u32, u32),
        layers: &[Layer<'_>],
        out: &mut Vec<u8>,
    ) -> Result<(), RenderError> {
        let _serial = software_guard(self.software);
        let mut encoder = self.render(size, layers)?;
        let target = self
            .target
            .as_ref()
            .ok_or_else(|| RenderError::Device("输出目标没建好".into()))?;
        copy_to_buffer(
            &mut encoder,
            &target.texture,
            &target.readback,
            target.padded_row,
            size,
        );
        self.queue.submit([encoder.finish()]);
        read_back(
            &self.device,
            &target.readback,
            target.padded_row,
            size.0 * 4,
            size.1,
            out,
        )
    }
}

impl Compositor {
    /// 合成一帧并在显卡上转成 BT.709 有限范围的 NV12，等它做完再返回（给测试和单帧用）。
    ///
    /// # Errors
    /// 输出尺寸为零或不是偶数、图层数据和尺寸对不上、读回失败。
    pub fn compose_nv12(
        &mut self,
        size: (u32, u32),
        layers: &[Layer<'_>],
        out: &mut Nv12,
    ) -> Result<(), RenderError> {
        self.submit_nv12(size, layers)?;
        // 之前还有在途的帧时，按顺序取到刚提交的这一帧为止
        while self.in_flight.len() > 1 {
            self.receive_nv12(out)?;
        }
        self.receive_nv12(out)
    }

    /// 已提交、还没取回的帧数。
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.in_flight.len()
    }

    /// 提交一帧 NV12 合成，不等显卡做完。之后用 [`Self::receive_nv12`] 按提交顺序取回。
    ///
    /// 导出时保持几帧在途：显卡忙的时候（比如在跑游戏）单帧来回要等很久，
    /// 流水起来以后吞吐只受显卡算力限制，不受单帧等待时间限制。
    ///
    /// # Errors
    /// 输出尺寸为零或不是偶数、图层数据和尺寸对不上。
    pub fn submit_nv12(
        &mut self,
        size: (u32, u32),
        layers: &[Layer<'_>],
    ) -> Result<(), RenderError> {
        if !size.0.is_multiple_of(2) || !size.1.is_multiple_of(2) {
            return Err(RenderError::BadLayer(format!(
                "NV12 要求宽高为偶数，实际 {}×{}",
                size.0, size.1
            )));
        }
        let _serial = software_guard(self.software);
        let mut encoder = self.render(size, layers)?;
        self.ensure_nv12(size);
        let readback = self.take_readback(size);
        let (Some(pipes), Some(t)) = (self.nv12.as_ref(), self.nv12_target.as_ref()) else {
            return Err(RenderError::Device("NV12 资源没建好".into()));
        };
        for (pipeline, texture) in [(&pipes.y, &t.y), (&pipes.uv, &t.uv)] {
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("nv12"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &t.bind, &[]);
            pass.draw(0..3, 0..1);
        }
        let half = (size.0 / 2, size.1 / 2);
        copy_to_buffer(&mut encoder, &t.y, &readback.y_buf, readback.row, size);
        copy_to_buffer(&mut encoder, &t.uv, &readback.uv_buf, readback.row, half);
        let submission = self.queue.submit([encoder.finish()]);
        let (tx, mapped) = std::sync::mpsc::channel();
        for buf in [&readback.y_buf, &readback.uv_buf] {
            let tx = tx.clone();
            buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        }
        self.in_flight.push_back(InFlight {
            readback,
            submission,
            mapped,
        });
        Ok(())
    }

    /// 取回最早提交的一帧，拷进 `out`（必要时等显卡做完）。
    ///
    /// # Errors
    /// 没有在途的帧，或读回失败。
    pub fn receive_nv12(&mut self, out: &mut Nv12) -> Result<(), RenderError> {
        let frame = self.receive_nv12_mapped()?;
        let (w, h) = frame.size();
        out.data.clear();
        out.data.reserve(w as usize * h as usize * 3 / 2);
        frame
            .write_to(&mut out.data)
            .map_err(|e| RenderError::Device(e.to_string()))?;
        out.width = w;
        out.height = h;
        self.recycle(frame);
        Ok(())
    }

    /// 取回最早提交的一帧，不拷贝，缓冲保持映射（必要时等显卡做完）。
    ///
    /// # Errors
    /// 没有在途的帧，或读回失败。
    pub fn receive_nv12_mapped(&mut self) -> Result<MappedNv12, RenderError> {
        let _serial = software_guard(self.software);
        let frame = self
            .in_flight
            .pop_front()
            .ok_or_else(|| RenderError::Device("没有在途的帧".into()))?;
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(frame.submission),
                timeout: None,
            })
            .map_err(|e| RenderError::Device(e.to_string()))?;
        for _ in 0..2 {
            frame
                .mapped
                .recv()
                .map_err(|e| RenderError::Device(e.to_string()))?
                .map_err(|e| RenderError::Device(e.to_string()))?;
        }
        Ok(MappedNv12 {
            readback: frame.readback,
        })
    }

    /// 写完的帧还回来：解除映射，缓冲放回池里给下一帧用。
    pub fn recycle(&mut self, frame: MappedNv12) {
        let _serial = software_guard(self.software);
        let rb = frame.readback;
        rb.y_buf.unmap();
        rb.uv_buf.unmap();
        self.readbacks.push(rb);
    }

    /// 取一套空闲的读回缓冲（尺寸不对的丢掉，没有就新建）。
    fn take_readback(&mut self, size: (u32, u32)) -> Readback {
        self.readbacks.retain(|r| r.size == size);
        if let Some(r) = self.readbacks.pop() {
            return r;
        }
        // Y：每像素 1 字节；UV：半宽，每点 2 字节，一行也是 width 字节
        let row = padded(size.0);
        let buffer = |label, rows: u32| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: u64::from(row) * u64::from(rows),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        Readback {
            size,
            y_buf: buffer("nv12-y", size.1),
            uv_buf: buffer("nv12-uv", size.1 / 2),
            row,
        }
    }

    fn ensure_nv12(&mut self, size: (u32, u32)) {
        if self.nv12.is_none() {
            self.nv12 = Some(nv12_pipelines(&self.device));
        }
        if self.nv12_target.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let (Some(pipes), Some(target)) = (self.nv12.as_ref(), self.target.as_ref()) else {
            return;
        };
        let plane = |label, format, dims: (u32, u32)| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent(dims),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let half = (size.0 / 2, size.1 / 2);
        let src = target
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nv12"),
            layout: &pipes.layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&src),
            }],
        });
        self.nv12_target = Some(Nv12Target {
            size,
            y: plane("nv12-y", wgpu::TextureFormat::R8Unorm, size),
            uv: plane("nv12-uv", wgpu::TextureFormat::Rg8Unorm, half),
            bind,
        });
    }
}

/// 从已映射的读回缓冲里按行拷出（去掉每行的对齐填充），追加到 `out`，然后解除映射。
fn copy_rows(
    buffer: &wgpu::Buffer,
    padded_row: u32,
    row: u32,
    rows: u32,
    out: &mut Vec<u8>,
) -> Result<(), RenderError> {
    {
        let mapped = buffer
            .slice(..)
            .get_mapped_range()
            .map_err(|e| RenderError::Device(e.to_string()))?;
        let (row, padded_row) = (row as usize, padded_row as usize);
        out.reserve(row * rows as usize);
        for chunk in mapped.chunks(padded_row).take(rows as usize) {
            out.extend_from_slice(&chunk[..row]);
        }
    }
    buffer.unmap();
    Ok(())
}

fn nv12_pipelines(device: &wgpu::Device) -> Nv12Pipelines {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("nv12"),
        source: wgpu::ShaderSource::Wgsl(include_str!("nv12.wgsl").into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("nv12"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("nv12"),
        bind_group_layouts: &[Some(&layout)],
        ..Default::default()
    });
    let make = |entry: &str, format| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(entry),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(entry),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        })
    };
    Nv12Pipelines {
        y: make("fs_y", wgpu::TextureFormat::R8Unorm),
        uv: make("fs_uv", wgpu::TextureFormat::Rg8Unorm),
        layout,
    }
}

/// 把纹理拷到读回缓冲（每行按 256 字节对齐）。
fn copy_to_buffer(
    encoder: &mut wgpu::CommandEncoder,
    texture: &wgpu::Texture,
    buffer: &wgpu::Buffer,
    padded_row: u32,
    size: (u32, u32),
) {
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: Some(size.1),
            },
        },
        extent(size),
    );
}

/// 等显卡做完，把读回缓冲里的每行（去掉对齐填充）拷到 `out`（先清空）。
fn read_back(
    device: &wgpu::Device,
    buffer: &wgpu::Buffer,
    padded_row: u32,
    row: u32,
    rows: u32,
    out: &mut Vec<u8>,
) -> Result<(), RenderError> {
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| RenderError::Device(e.to_string()))?;
    rx.recv()
        .map_err(|e| RenderError::Device(e.to_string()))?
        .map_err(|e| RenderError::Device(e.to_string()))?;
    out.clear();
    copy_rows(buffer, padded_row, row, rows, out)
}

const fn extent(size: (u32, u32)) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: size.0,
        height: size.1,
        depth_or_array_layers: 1,
    }
}

/// 读回时每行要按 256 字节对齐。
const fn padded(bytes: u32) -> u32 {
    bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT
}

#[expect(clippy::cast_possible_truncation, reason = "NDC 坐标在 ±几十以内")]
const fn to_f32(v: f64) -> f32 {
    v as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use ve_core::{LayerGeometry, Rect};

    const SIZE: (u32, u32) = (64, 36);

    fn compositor() -> Compositor {
        Compositor::new(&RenderConfig { software: true }).unwrap()
    }

    /// 纯色 BGRA（预乘）。
    fn solid(w: u32, h: u32, bgra: [u8; 4]) -> Vec<u8> {
        bgra.repeat((w * h) as usize)
    }

    fn geom(x: f32, y: f32, w: f32, h: f32) -> LayerGeometry {
        LayerGeometry {
            full: Rect { x, y, w, h },
            shown: Rect { x, y, w, h },
            uv: Rect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
        }
    }

    fn layer(data: &[u8], w: u32, h: u32, geometry: LayerGeometry, opacity: f32) -> Layer<'_> {
        Layer {
            pixels: FramePixels::Bgra {
                width: w,
                height: h,
                data,
            },
            geometry,
            opacity,
        }
    }

    fn px(out: &[u8], x: u32, y: u32) -> [u8; 4] {
        let i = ((y * SIZE.0 + x) * 4) as usize;
        [out[i], out[i + 1], out[i + 2], out[i + 3]]
    }

    fn near(got: [u8; 4], want: [u8; 3], tol: u8) -> bool {
        got[..3].iter().zip(want).all(|(g, w)| g.abs_diff(w) <= tol)
    }

    const FULL: LayerGeometry = LayerGeometry {
        full: Rect {
            x: 0.0,
            y: 0.0,
            w: 64.0,
            h: 36.0,
        },
        shown: Rect {
            x: 0.0,
            y: 0.0,
            w: 64.0,
            h: 36.0,
        },
        uv: Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        },
    };

    #[test]
    fn empty_is_black() {
        let mut c = compositor();
        let mut out = Vec::new();
        c.compose_bgra(SIZE, &[], &mut out).unwrap();
        assert_eq!(out.len(), 64 * 36 * 4);
        assert!(out.as_chunks::<4>().0.iter().all(|p| p[..3] == [0, 0, 0]));
        assert_ne!(c.adapter_name(), "");
    }

    #[test]
    fn opaque_layer_lands_on_exact_pixels() {
        let mut c = compositor();
        let red = solid(4, 4, [0, 0, 255, 255]);
        let mut out = Vec::new();
        c.compose_bgra(
            SIZE,
            &[layer(&red, 4, 4, geom(16.0, 8.0, 32.0, 18.0), 1.0)],
            &mut out,
        )
        .unwrap();
        assert!(near(px(&out, 16, 8), [0, 0, 255], 0));
        assert!(near(px(&out, 47, 25), [0, 0, 255], 0));
        assert!(near(px(&out, 15, 10), [0, 0, 0], 0));
        assert!(near(px(&out, 48, 25), [0, 0, 0], 0));
        assert!(near(px(&out, 20, 7), [0, 0, 0], 0));
        assert!(near(px(&out, 20, 26), [0, 0, 0], 0));
    }

    #[test]
    fn opacity_blends() {
        let mut c = compositor();
        let white = solid(2, 2, [255; 4]);
        let black = solid(2, 2, [0, 0, 0, 255]);
        let mut out = Vec::new();
        c.compose_bgra(
            SIZE,
            &[
                layer(&white, 2, 2, FULL, 1.0),
                layer(&black, 2, 2, FULL, 0.5),
            ],
            &mut out,
        )
        .unwrap();
        assert!(
            near(px(&out, 32, 18), [128, 128, 128], 1),
            "{:?}",
            px(&out, 32, 18)
        );
    }

    #[test]
    fn crop_uv_samples_subregion() {
        let mut c = compositor();
        // 4×2：左半红、右半蓝
        let mut tex = Vec::new();
        for _ in 0..2 {
            tex.extend(solid(2, 1, [0, 0, 255, 255]));
            tex.extend(solid(2, 1, [255, 0, 0, 255]));
        }
        let mut g = FULL;
        g.uv = Rect {
            x: 0.5,
            y: 0.0,
            w: 0.5,
            h: 1.0,
        };
        let mut out = Vec::new();
        c.compose_bgra(SIZE, &[layer(&tex, 4, 2, g, 1.0)], &mut out)
            .unwrap();
        assert!(
            near(px(&out, 32, 18), [255, 0, 0], 2),
            "{:?}",
            px(&out, 32, 18)
        );
        assert!(near(px(&out, 60, 18), [255, 0, 0], 2));
    }

    #[test]
    fn premultiplied_edges_not_dark() {
        let mut c = compositor();
        let white = solid(2, 2, [255; 4]);
        // 半透明红，预乘后 (B, G, R, A) = (0, 0, 128, 128)
        let half_red = solid(2, 2, [0, 0, 128, 128]);
        let mut out = Vec::new();
        c.compose_bgra(
            SIZE,
            &[
                layer(&white, 2, 2, FULL, 1.0),
                layer(&half_red, 2, 2, FULL, 1.0),
            ],
            &mut out,
        )
        .unwrap();
        assert!(
            near(px(&out, 32, 18), [127, 127, 255], 1),
            "{:?}",
            px(&out, 32, 18)
        );
    }

    #[test]
    fn layer_resize_reallocates() {
        let mut c = compositor();
        let red = solid(4, 4, [0, 0, 255, 255]);
        let blue = solid(8, 6, [255, 0, 0, 255]);
        let mut out = Vec::new();
        c.compose_bgra(SIZE, &[layer(&red, 4, 4, FULL, 1.0)], &mut out)
            .unwrap();
        c.compose_bgra(SIZE, &[layer(&blue, 8, 6, FULL, 1.0)], &mut out)
            .unwrap();
        assert!(near(px(&out, 32, 18), [255, 0, 0], 0));
        // 输出尺寸也能变
        c.compose_bgra((32, 18), &[layer(&blue, 8, 6, FULL, 1.0)], &mut out)
            .unwrap();
        assert_eq!(out.len(), 32 * 18 * 4);
    }

    #[test]
    fn size_mismatch_is_error() {
        let mut c = compositor();
        let short = vec![0u8; 10];
        let mut out = Vec::new();
        assert!(matches!(
            c.compose_bgra(SIZE, &[layer(&short, 4, 4, FULL, 1.0)], &mut out),
            Err(RenderError::BadLayer(_))
        ));
        assert!(matches!(
            c.compose_bgra((0, 36), &[], &mut out),
            Err(RenderError::BadLayer(_))
        ));
    }
    /// BT.709 有限范围：RGB（0–255）→ (Y, Cb, Cr)，四舍五入。
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "有限范围的结果在 16–240 之间"
    )]
    fn bt709(red: f64, green: f64, blue: f64) -> [u8; 3] {
        let (r, g, b) = (red / 255.0, green / 255.0, blue / 255.0);
        let luma = 219.0f64.mul_add(0.0722f64.mul_add(b, 0.2126f64.mul_add(r, 0.7152 * g)), 16.0);
        let cb = 224.0f64.mul_add(
            0.5f64.mul_add(b, (-0.114_572f64).mul_add(r, -0.385_428 * g)),
            128.0,
        );
        let cr = 224.0f64.mul_add(
            (-0.045_847f64).mul_add(b, 0.5f64.mul_add(r, -0.454_153 * g)),
            128.0,
        );
        [luma.round() as u8, cb.round() as u8, cr.round() as u8]
    }

    fn nv12_of(bgra: [u8; 4]) -> Nv12 {
        let mut c = compositor();
        let tex = solid(2, 2, bgra);
        let mut out = Nv12::default();
        c.compose_nv12(SIZE, &[layer(&tex, 2, 2, FULL, 1.0)], &mut out)
            .unwrap();
        out
    }

    /// 中心附近的 (Y, U, V)。
    fn yuv_at_center(frame: &Nv12) -> [u8; 3] {
        let (width, height) = (frame.width as usize, frame.height as usize);
        let (col, row) = (width / 2, height / 2);
        let luma = frame.data[row * width + col];
        let uv = width * height + (row / 2) * width + (col / 2) * 2;
        [luma, frame.data[uv], frame.data[uv + 1]]
    }

    #[test]
    fn nv12_matches_bt709_formula() {
        let n = nv12_of([50, 100, 200, 255]);
        assert_eq!((n.width, n.height), SIZE);
        assert_eq!(n.data.len(), 64 * 36 * 3 / 2);
        let got = yuv_at_center(&n);
        let want = bt709(200.0, 100.0, 50.0);
        for (g, w) in got.iter().zip(want) {
            assert!(g.abs_diff(w) <= 1, "期望 {want:?}，实际 {got:?}");
        }
    }

    #[test]
    fn nv12_black_and_white() {
        assert_eq!(yuv_at_center(&nv12_of([0, 0, 0, 255])), [16, 128, 128]);
        assert_eq!(yuv_at_center(&nv12_of([255; 4])), [235, 128, 128]);
        // 空画面就是黑场
        let mut c = compositor();
        let mut out = Nv12::default();
        c.compose_nv12(SIZE, &[], &mut out).unwrap();
        assert_eq!(yuv_at_center(&out), [16, 128, 128]);
    }

    #[test]
    fn nv12_chroma_averages_2x2() {
        // 左半红、右半蓝，画布 4×2：UV 平面 2×1，各取对应 2×2 块的平均
        let mut c = compositor();
        let mut tex = Vec::new();
        for _ in 0..2 {
            tex.extend(solid(2, 1, [0, 0, 255, 255]));
            tex.extend(solid(2, 1, [255, 0, 0, 255]));
        }
        let g = LayerGeometry {
            full: Rect {
                x: 0.0,
                y: 0.0,
                w: 4.0,
                h: 2.0,
            },
            shown: Rect {
                x: 0.0,
                y: 0.0,
                w: 4.0,
                h: 2.0,
            },
            uv: Rect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
        };
        let mut out = Nv12::default();
        c.compose_nv12((4, 2), &[layer(&tex, 4, 2, g, 1.0)], &mut out)
            .unwrap();
        let red = bt709(255.0, 0.0, 0.0);
        let blue = bt709(0.0, 0.0, 255.0);
        assert!(out.data[0].abs_diff(red[0]) <= 1 && out.data[3].abs_diff(blue[0]) <= 1);
        let uv = &out.data[8..];
        assert!(
            uv[0].abs_diff(red[1]) <= 2 && uv[1].abs_diff(red[2]) <= 2,
            "{uv:?}"
        );
        assert!(
            uv[2].abs_diff(blue[1]) <= 2 && uv[3].abs_diff(blue[2]) <= 2,
            "{uv:?}"
        );
    }

    #[test]
    fn odd_size_rejected() {
        let mut c = compositor();
        let mut out = Nv12::default();
        assert!(matches!(
            c.compose_nv12((63, 36), &[], &mut out),
            Err(RenderError::BadLayer(_))
        ));
    }

    #[test]
    fn pipelined_nv12_keeps_order_and_matches_sync() {
        // 连续提交几帧再按顺序取回：结果和逐帧同步合成一样，顺序不乱
        let mut c = compositor();
        let colors: Vec<[u8; 4]> = (0..5u8).map(|i| [i * 50, 100, 200 - i * 40, 255]).collect();
        let texes: Vec<Vec<u8>> = colors.iter().map(|&bgra| solid(2, 2, bgra)).collect();
        let mut expected = Vec::new();
        for t in &texes {
            let mut n = Nv12::default();
            c.compose_nv12(SIZE, &[layer(t, 2, 2, FULL, 1.0)], &mut n)
                .unwrap();
            expected.push(n);
        }
        let mut got = Vec::new();
        for t in &texes {
            c.submit_nv12(SIZE, &[layer(t, 2, 2, FULL, 1.0)]).unwrap();
            if c.in_flight() >= 3 {
                let mut n = Nv12::default();
                c.receive_nv12(&mut n).unwrap();
                got.push(n);
            }
        }
        while c.in_flight() > 0 {
            let mut n = Nv12::default();
            c.receive_nv12(&mut n).unwrap();
            got.push(n);
        }
        assert_eq!(got, expected);
        // 没有在途的帧时取不到
        let mut n = Nv12::default();
        assert!(matches!(
            c.receive_nv12(&mut n),
            Err(RenderError::Device(_))
        ));
    }

    #[test]
    fn mapped_frame_writes_tight_nv12_and_recycles() {
        // 读回缓冲直接写出去（不先拷进 Vec），写出的字节和 receive_nv12 一样；用完放回池里
        let mut c = compositor();
        let tex = solid(2, 2, [50, 100, 200, 255]);
        let mut want = Nv12::default();
        c.compose_nv12(SIZE, &[layer(&tex, 2, 2, FULL, 1.0)], &mut want)
            .unwrap();
        c.submit_nv12(SIZE, &[layer(&tex, 2, 2, FULL, 1.0)])
            .unwrap();
        let mapped = c.receive_nv12_mapped().unwrap();
        assert_eq!(mapped.size(), SIZE);
        let mut bytes = Vec::new();
        mapped.write_to(&mut bytes).unwrap();
        assert_eq!(bytes, want.data);
        c.recycle(mapped);
        // 回收后的缓冲能再用
        let mut again = Nv12::default();
        c.compose_nv12(SIZE, &[layer(&tex, 2, 2, FULL, 1.0)], &mut again)
            .unwrap();
        assert_eq!(again, want);
    }

    #[test]
    fn oversized_layer_is_an_error_not_a_panic() {
        let mut c = compositor();
        let max = c.max_texture_size();
        assert!(max >= 2048, "{max}");
        let wide = max + 8;
        let data = vec![255u8; wide as usize * 4];
        let mut out = Vec::new();
        assert!(matches!(
            c.compose_bgra(SIZE, &[layer(&data, wide, 1, FULL, 1.0)], &mut out),
            Err(RenderError::BadLayer(_))
        ));
        assert!(matches!(
            c.compose_bgra((wide, 2), &[], &mut out),
            Err(RenderError::BadLayer(_))
        ));
        // 之后还能正常用
        c.compose_bgra(SIZE, &[], &mut out).unwrap();
    }
}
