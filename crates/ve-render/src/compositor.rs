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
    /// 调色（单位矩阵时着色器跳过）。
    pub color: ve_core::ColorMatrix,
    /// 外观：混合模式、圆角、描边、阴影、蒙版、色度抠图。
    pub look: ve_core::Look,
    /// 整条调色链（用到进阶调色时才有）：烘焙成 3D 查找表逐像素查，不再用 `color`。
    pub grades: &'a [ve_core::GradeStep],
    /// 锐化、颗粒、暗角。
    pub detail: ve_core::Detail,
    /// 文字蒙版的字（外观里有文字形状的蒙版时才有）。
    pub mask_text: Option<&'a ve_core::MaskText>,
}

/// 另外几个蒙版（[`ve_core::Look::extra_masks`]）：每个实例一条，放在存储缓冲里（实例顶点属性的
/// 16 个槽已经用满）。第 0 个 `vec4` 的 x 是有几个，之后每个蒙版三个 `vec4`：
/// （中心 x、y、半宽、半高）、（旋转、羽化、圆角、反转）、（形状，0，0，0）。
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ExtraMasks {
    v: [[f32; 4]; 1 + 3 * (ve_core::look::MAX_MASKS - 1)],
}

/// 蒙版换算成着色器要的（形状编号，中心和半宽高，旋转羽化圆角反转）。
fn mask_params(m: &ve_core::Mask, w: f32, h: f32) -> (f32, [f32; 4], [f32; 4]) {
    let half_min = w.min(h) / 2.0;
    let (hw, hh) = (m.width * w / 2.0, m.height * h / 2.0);
    #[expect(clippy::cast_precision_loss, reason = "形状编号很小")]
    let shape = m.shape.code() as f32;
    (
        shape,
        [w * (0.5 + m.x), h * (0.5 + m.y), hw, hh],
        [
            m.rotation.to_radians(),
            m.feather * half_min,
            m.round * hw.min(hh),
            if m.invert { 1.0 } else { 0.0 },
        ],
    )
}

/// 分组那一道的参数（`group.wgsl` 的 `Amount`）：不透明度；有形状时整组按画布大小的图层放在
/// 输出（`size`）上，算出转之前的显示框、旋转、圆角和蒙版（和图层的蒙版同一套换算）。
fn group_params(g: &ve_core::compose::GroupPass, size: (u32, u32)) -> [[f32; 4]; 15] {
    let mut v = [[0.0; 4]; 15];
    v[0][0] = g.opacity.clamp(0.0, 1.0);
    v[2][2] = -1.0;
    let Some(shape) = g.shape else {
        return v;
    };
    let geo = ve_core::layer_geometry(size, size, &shape.transform);
    let (w, h) = (geo.shown.w, geo.shown.h);
    let look = &shape.look;
    v[0] = [v[0][0], 1.0, geo.angle, look.radius * w.min(h) / 2.0];
    v[1] = [geo.center.0, geo.center.1, geo.shown.x, geo.shown.y];
    if let Some(m) = look.mask {
        let (code, center, more) = mask_params(&m, w, h);
        v[2] = [w, h, code, 0.0];
        v[3] = center;
        v[4] = more;
    } else {
        v[2] = [w, h, -1.0, 0.0];
    }
    let extras = extra_masks(look, w, h);
    v[5..].copy_from_slice(&extras.v);
    v
}

/// 一层另外几个蒙版的记录。
fn extra_masks(look: &ve_core::Look, w: f32, h: f32) -> ExtraMasks {
    let mut out = ExtraMasks::zeroed();
    let mut used = 0usize;
    for m in look.extra_masks.iter().flatten() {
        let (shape, center, more) = mask_params(m, w, h);
        out.v[1 + used * 3] = center;
        out.v[2 + used * 3] = more;
        out.v[3 + used * 3] = [shape, 0.0, 0.0, 0.0];
        used += 1;
    }
    #[expect(clippy::cast_precision_loss, reason = "最多三个")]
    let count = used as f32;
    out.v[0] = [count, 0.0, 0.0, 0.0];
    out
}

/// 每层的参数，作为实例顶点数据传给着色器。
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LayerInstance {
    /// 左上、右上两个角（NDC）。转过的图层四个角不再是正矩形，所以逐个角给。
    top: [f32; 4],
    /// 左下、右下两个角（NDC）。
    bottom: [f32; 4],
    uv: [f32; 4],
    /// 不透明度、要不要调色（1 / 0）、混合模式编号、圆角半径（像素）。
    p0: [f32; 4],
    /// 调色矩阵的三行（输出 r、g、b = 行 · (r, g, b, 1)）。
    red: [f32; 4],
    green: [f32; 4],
    blue: [f32; 4],
    /// 显示部分宽、高（像素），往外扩的像素，描边粗细（像素）。
    sizes: [f32; 4],
    /// 描边颜色。
    stroke: [f32; 4],
    /// 阴影颜色、不透明度。
    shadow: [f32; 4],
    /// 阴影偏移 xy（图层坐标）、模糊、蒙版形状（-1 没有）。
    shadow2: [f32; 4],
    /// 蒙版中心 xy（图层坐标）、半宽、半高。
    mask: [f32; 4],
    /// 蒙版旋转（弧度）、羽化、圆角（像素）、反转。
    mask2: [f32; 4],
    /// 抠像颜色、低阈值。
    key: [f32; 4],
    /// 高阈值、开没开。
    key2: [f32; 4],
    /// 锐化、颗粒、暗角、颗粒种子。
    detail: [f32; 4],
}

const INSTANCE_ATTRS: [wgpu::VertexAttribute; 16] = wgpu::vertex_attr_array![
    0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4,
    4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4,
    8 => Float32x4, 9 => Float32x4, 10 => Float32x4, 11 => Float32x4,
    12 => Float32x4, 13 => Float32x4, 14 => Float32x4, 15 => Float32x4
];

/// 外观换成着色器参数（`size` 是显示部分的像素大小，`short` 是画布短边，`angle` 是图层旋转角）。
#[expect(clippy::many_single_char_names, reason = "宽高、颜色分量")]
fn look_params(look: &ve_core::Look, size: (f32, f32), short: f32, angle: f32) -> LookParams {
    let (w, h) = size;
    let half_min = w.min(h) / 2.0;
    let rgb = |c: u32| {
        let [r, g, b] = ve_core::look::rgb_unit(c);
        [r, g, b, 1.0]
    };
    let stroke_w = look.stroke.map_or(0.0, |s| s.width * short);
    let (shadow, shadow_off, blur) = look.shadow.map_or(([0.0; 4], (0.0, 0.0), 0.0), |s| {
        let dist = s.distance * short;
        let a = s.angle.to_radians();
        // 画布上的方向转回图层自己的方向（图层转了多少，影子方向就要反着转回来）
        let (dx, dy) = (dist * a.cos(), dist * a.sin());
        let (sin, cos) = (-angle).sin_cos();
        let off = (dx * cos - dy * sin, dx * sin + dy * cos);
        let [r, g, b, _] = rgb(s.color);
        ([r, g, b, s.opacity], off, s.blur * short)
    });
    let (mask_shape, mask, mask2) = look
        .mask
        .map_or((-1.0, [0.0; 4], [0.0; 4]), |m| mask_params(&m, w, h));
    let (key, key2) = look.chroma.map_or(([0.0; 4], [0.0; 4]), |k| {
        let (lo, hi) = k.thresholds();
        let [r, g, b, _] = rgb(k.color);
        ([r, g, b, lo], [hi, 1.0, 0.0, 0.0])
    });
    let expand = if stroke_w > 0.0 || shadow[3] > 0.0 {
        stroke_w.max(shadow_off.0.hypot(shadow_off.1) + blur * 2.0) + 2.0
    } else {
        0.0
    };
    let stroke = look.stroke.map_or([0.0; 4], |s| rgb(s.color));
    LookParams {
        radius: look.radius * half_min,
        sizes: [w, h, expand, stroke_w],
        stroke,
        shadow,
        shadow2: [shadow_off.0, shadow_off.1, blur, mask_shape],
        mask,
        mask2,
        key,
        key2,
    }
}

/// [`look_params`] 的结果。
struct LookParams {
    radius: f32,
    sizes: [f32; 4],
    stroke: [f32; 4],
    shadow: [f32; 4],
    shadow2: [f32; 4],
    mask: [f32; 4],
    mask2: [f32; 4],
    key: [f32; 4],
    key2: [f32; 4],
}

/// 一个图层槽位：纹理按尺寸复用。
struct Slot {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
    bind: wgpu::BindGroup,
    /// 绑着哪段字的文字距离场（`None` 绑的是占位）。
    text: Option<ve_core::MaskText>,
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
    /// 混合模式图层要读的底图：画它之前把已经合成好的画面拷进来。
    backdrop: wgpu::Texture,
    backdrop_bind: wgpu::BindGroup,
    /// 半透明复合片段每组开始前拍下的画面（一组一张，按 [`Compositor::groups`] 的顺序）。
    snapshots: Vec<(wgpu::Texture, wgpu::BindGroup)>,
}

/// 显卡合成器。
pub struct Compositor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    /// 混合模式不是「正常」的图层：读底图、算好后直接写回。
    blend_pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    backdrop_layout: wgpu::BindGroupLayout,
    lut_layout: wgpu::BindGroupLayout,
    extras_layout: wgpu::BindGroupLayout,
    lut_sampler: wgpu::Sampler,
    /// 不用查找表的图层绑的占位表。
    no_lut: wgpu::BindGroup,
    /// 烘焙过的查找表（按调色链的内容），满了就清空重来。
    luts: std::collections::HashMap<u64, wgpu::BindGroup>,
    /// 这一帧各层用哪个查找表（`None` 用占位表）。
    layer_luts: Vec<Option<u64>>,
    /// 这一帧各层要不要走混合模式那条路。
    blended: Vec<bool>,
    sampler: wgpu::Sampler,
    slots: Vec<Slot>,
    /// 各层参数（实例顶点缓冲）和容量，不够时重建。
    instances: Option<(wgpu::Buffer, usize)>,
    /// 另外几个蒙版的存储缓冲、它的绑定组和容量（条数），不够时重建。
    extras: Option<(wgpu::Buffer, wgpu::BindGroup, usize)>,
    target: Option<Target>,
    nv12: Option<Nv12Pipelines>,
    nv12_target: Option<Nv12Target>,
    readbacks: Vec<Readback>,
    in_flight: std::collections::VecDeque<InFlight>,
    adapter: String,
    software: bool,
    /// 底色（没有画面的地方），默认黑；调节片段会把它也调了。
    background: wgpu::Color,
    /// 画面特效（[`Self::set_effects`]）。
    effects: Vec<ve_core::EffectPass>,
    /// 画面特效那一道：读底图、整个画面重画。
    effect_pipeline: wgpu::RenderPipeline,
    /// 特效那一道不读图层纹理，第 0 组绑个 1×1 的占位。
    dummy_layer: wgpu::BindGroup,
    /// 没有文字蒙版的图层在文字距离场那个位置绑的 1×1 占位。
    no_text: wgpu::TextureView,
    /// 算好的文字距离场（按字和字体），满了就清空重来；算不出来的记 `None`。
    text_fields: std::collections::HashMap<ve_core::MaskText, Option<wgpu::TextureView>>,
    /// 插件特效的管线（和同一块显卡上的合成器共用）。
    plugins: PluginCache,
    /// 半透明复合片段的分组（[`Self::set_groups`]）。
    groups: Vec<ve_core::compose::GroupPass>,
    /// 分组那一道：快照和画好的画面按整组不透明度混合。
    group_pipeline: wgpu::RenderPipeline,
    /// 分组那一道的不透明度（uniform）布局。
    group_layout: wgpu::BindGroupLayout,
    /// 这一帧各组的不透明度绑定。
    group_binds: Vec<wgpu::BindGroup>,
}

impl Drop for Compositor {
    /// 软件显卡上释放资源也要和别的合成器错开（同上：WARP 并发会崩）。设备和管线随后自动释放。
    fn drop(&mut self) {
        let _serial = software_guard(self.software);
        self.in_flight.clear();
        self.readbacks.clear();
        self.nv12_target = None;
        self.target = None;
        self.instances = None;
        self.extras = None;
        self.slots.clear();
        self.luts.clear();
    }
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
    // 没被错误范围接住的错误：记日志，不 panic（默认处理会让整个程序崩掉）
    device.on_uncaptured_error(std::sync::Arc::new(|e: wgpu::Error| {
        log::error!("显卡错误：{e}");
    }));
    Ok((
        device,
        queue,
        name,
        info.device_type == wgpu::DeviceType::Cpu,
    ))
}

/// 等显卡最多这么久（正常一帧远用不了这么久）；超时报错，别让预览、导出永远挂住。
const GPU_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// 整个进程共用的显卡设备和管线（硬件一套、软件一套）：建一次要几百毫秒，
/// 而且几十个线程同时建 DX12 设备实测偶尔会让整个进程卡死（测试里）。
#[derive(Clone)]
struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    blend_pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    backdrop_layout: wgpu::BindGroupLayout,
    lut_layout: wgpu::BindGroupLayout,
    extras_layout: wgpu::BindGroupLayout,
    lut_sampler: wgpu::Sampler,
    no_lut: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    effect_pipeline: wgpu::RenderPipeline,
    dummy_layer: wgpu::BindGroup,
    /// 编译好的插件特效（按插件名和源码）；编译失败的记 `None`，不再试。
    plugins: PluginCache,
    adapter: String,
    software: bool,
    /// 设备丢了（驱动重置等）：之后再要设备时重建一套。
    lost: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// 插件管线缓存：（插件名，源码散列）→ 管线。
type PluginCache = std::sync::Arc<
    std::sync::Mutex<std::collections::HashMap<(String, u64), Option<wgpu::RenderPipeline>>>,
>;

/// 共用的一套现在的状态。
enum Shared {
    /// 还没建，或者上次建失败了 / 设备丢了（下次再建）。
    None,
    Gpu(Gpu),
    /// 要硬件显卡却只有软件显卡：用软件那一套。
    FellBack,
}

/// 取共用的那一套；第一次时创建。`software` 为假但机器上只有软件显卡时，用软件那一套。
/// 建失败不记住（下次再试）；设备丢了（驱动重置）下次重建。
#[expect(
    clippy::significant_drop_tightening,
    reason = "建设备时要一直拿着锁：别的线程等这一套建好，不同时建"
)]
fn shared_gpu(software: bool) -> Result<Gpu, RenderError> {
    static HARDWARE: std::sync::Mutex<Shared> = std::sync::Mutex::new(Shared::None);
    static SOFTWARE: std::sync::Mutex<Shared> = std::sync::Mutex::new(Shared::None);
    let lock = |m: &'static std::sync::Mutex<Shared>| {
        m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    };
    let alive = |g: &Gpu| !g.lost.load(std::sync::atomic::Ordering::Acquire);
    if !software {
        let mut hw = lock(&HARDWARE);
        match &*hw {
            Shared::Gpu(g) if alive(g) => return Ok(g.clone()),
            Shared::FellBack => {}
            _ => {
                // 建设备也串行（几十个线程同时建 DX12 设备会卡死；万一选到的是 WARP 也不并行）
                let made = {
                    let _serial = software_guard(true);
                    create_device(false).map(build_gpu)
                };
                match made {
                    Ok(g) if !g.software => {
                        *hw = Shared::Gpu(g.clone());
                        return Ok(g);
                    }
                    // 退回了软件显卡：丢掉这一个，和软件模式共用一套
                    Ok(_) => *hw = Shared::FellBack,
                    Err(e) => {
                        *hw = Shared::None;
                        return Err(e);
                    }
                }
            }
        }
    }
    let mut sw = lock(&SOFTWARE);
    if let Shared::Gpu(g) = &*sw
        && alive(g)
    {
        return Ok(g.clone());
    }
    let made = {
        let _serial = software_guard(true);
        create_device(true).map(build_gpu)
    };
    *sw = made
        .as_ref()
        .map_or(Shared::None, |g| Shared::Gpu(g.clone()));
    made
}

/// 在设备上建好合成要用的管线、绑定布局和采样器。
fn build_gpu((device, queue, adapter, cpu): DeviceParts) -> Gpu {
    let lost = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    device.set_device_lost_callback({
        let lost = std::sync::Arc::clone(&lost);
        move |reason, message| {
            log::error!("显卡设备丢失（{reason:?}）：{message}");
            lost.store(true, std::sync::atomic::Ordering::Release);
        }
    });
    let layout = layer_bind_layout(&device);
    let backdrop_layout = backdrop_bind_layout(&device);
    let lut_layout = lut_bind_layout(&device);
    let extras_layout = extras_bind_layout(&device);
    let layouts = [&layout, &backdrop_layout, &lut_layout, &extras_layout];
    let pipeline = compose_pipeline(&device, layouts, Pass::Normal);
    let blend_pipeline = compose_pipeline(&device, layouts, Pass::Blend);
    let effect_pipeline = compose_pipeline(&device, layouts, Pass::Effect);
    let lut_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("lut"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let no_lut = lut_bind(
        &device,
        &queue,
        &lut_layout,
        &lut_sampler,
        2,
        &[255; 2 * 2 * 2 * 4],
    );
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("layer"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let dummy = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("dummy-layer"),
        size: extent((1, 1)),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let dummy_view = dummy.create_view(&wgpu::TextureViewDescriptor::default());
    let dummy_layer = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("dummy-layer"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&dummy_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&dummy_view),
            },
        ],
    });
    Gpu {
        device,
        queue,
        pipeline,
        blend_pipeline,
        layout,
        backdrop_layout,
        lut_layout,
        extras_layout,
        lut_sampler,
        no_lut,
        sampler,
        effect_pipeline,
        dummy_layer,
        plugins: PluginCache::default(),
        adapter,
        software: cpu,
        lost,
    }
}

/// 整个进程共用一个实例（显卡驱动只加载一次）。
fn instance() -> wgpu::Instance {
    static INSTANCE: std::sync::OnceLock<wgpu::Instance> = std::sync::OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
            // Windows 上用 DX12：实测比 Vulkan 略快，系统自带，而且有 WARP 软件显卡可用于测试
            if cfg!(windows) {
                desc.backends = wgpu::Backends::DX12;
            }
            wgpu::Instance::new(desc)
        })
        .clone()
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
            // 文字蒙版的距离场（[`crate::text_mask`]）
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
        ],
    })
}

/// 另外几个蒙版的存储缓冲（第 3 组）。
fn extras_bind_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("extra-masks"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    })
}

fn lut_bind_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("lut"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D3,
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

/// 一个 3D 查找表（RGBA8，`size`³）的绑定组。
fn lut_bind(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    size: u32,
    data: &[u8],
) -> wgpu::BindGroup {
    let extent = wgpu::Extent3d {
        width: size,
        height: size,
        depth_or_array_layers: size,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("lut"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size * 4),
            rows_per_image: Some(size),
        },
        extent,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("lut"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

/// 调色链的指纹（缓存查找表用）。
fn chain_key(grades: &[ve_core::GradeStep]) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{grades:?}").hash(&mut h);
    h.finish()
}

/// 最多缓存这么多个查找表。
const LUT_CACHE: usize = 64;

fn backdrop_bind_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("backdrop"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    })
}

/// 合成管线：`blend` 为假是普通的预乘透明叠加（入口 `fs`），为真是混合模式（入口 `fs_blend`，
/// 读底图、直接写回不再叠加）。两条管线用同一个布局（第 1 组是底图，普通管线不读它）。
/// 合成管线的三种画法。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    /// 普通图层：预乘透明叠上去。
    Normal,
    /// 混合模式图层：读底图，算好直接写回。
    Blend,
    /// 画面特效：读底图，整个画面重画。
    Effect,
}

fn compose_pipeline(
    device: &wgpu::Device,
    layouts: [&wgpu::BindGroupLayout; 4],
    pass: Pass,
) -> wgpu::RenderPipeline {
    let entry = match pass {
        Pass::Normal => "fs",
        Pass::Blend => "fs_blend",
        Pass::Effect => "fs_effect",
    };
    pipeline_from(
        device,
        layouts,
        crate::LAYER_SHADER,
        entry,
        pass == Pass::Normal,
    )
}

/// 用源码 `source` 的入口 `entry` 建一条合成管线（`blend` 为真时预乘透明叠加，否则直接写回）。
fn pipeline_from(
    device: &wgpu::Device,
    layouts: [&wgpu::BindGroupLayout; 4],
    source: &str,
    entry: &str,
    blend: bool,
) -> wgpu::RenderPipeline {
    let [layout, backdrop, lut, extras] = layouts;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("compose"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("compose"),
        bind_group_layouts: &[Some(layout), Some(backdrop), Some(lut), Some(extras)],
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
            entry_point: Some(entry),
            targets: &[Some(wgpu::ColorTargetState {
                format: FORMAT,
                blend: blend.then_some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
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

/// 分组那一道的参数：一个 uniform（[`group_params`]）。
fn group_bind_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("group"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    })
}

/// 分组那一道的管线：读快照（第 0 组）和画好的画面（第 1 组），按不透明度（第 2 组）混合后直接写回。
fn group_pipeline(
    device: &wgpu::Device,
    backdrop: &wgpu::BindGroupLayout,
    amount: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("group"),
        source: wgpu::ShaderSource::Wgsl(crate::GROUP_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("group"),
        bind_group_layouts: &[Some(backdrop), Some(backdrop), Some(amount)],
        ..Default::default()
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("group"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            targets: &[Some(wgpu::ColorTargetState {
                format: FORMAT,
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
}

impl Compositor {
    /// 创建合成器（选显卡、编译着色器）。
    ///
    /// # Errors
    /// 找不到显卡，或创建设备失败。
    pub fn new(config: &RenderConfig) -> Result<Self, RenderError> {
        let Gpu {
            device,
            queue,
            pipeline,
            blend_pipeline,
            layout,
            backdrop_layout,
            lut_layout,
            extras_layout,
            lut_sampler,
            no_lut,
            sampler,
            effect_pipeline,
            dummy_layer,
            plugins,
            adapter,
            software,
            lost: _,
        } = shared_gpu(config.software)?;
        let no_text = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("no-text-mask"),
                size: extent((1, 1)),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        let group_layout = group_bind_layout(&device);
        let group_pipeline = group_pipeline(&device, &backdrop_layout, &group_layout);
        Ok(Self {
            device,
            queue,
            pipeline,
            blend_pipeline,
            layout,
            backdrop_layout,
            lut_layout,
            extras_layout,
            lut_sampler,
            no_lut,
            luts: std::collections::HashMap::new(),
            layer_luts: Vec::new(),
            blended: Vec::new(),
            sampler,
            slots: Vec::new(),
            instances: None,
            extras: None,
            target: None,
            nv12: None,
            nv12_target: None,
            readbacks: Vec::new(),
            in_flight: std::collections::VecDeque::new(),
            adapter,
            software,
            background: wgpu::Color::BLACK,
            effects: Vec::new(),
            effect_pipeline,
            dummy_layer,
            no_text,
            text_fields: std::collections::HashMap::new(),
            plugins,
            groups: Vec::new(),
            group_pipeline,
            group_layout,
            group_binds: Vec::new(),
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

    /// 一层的第 0 组：画面、采样器、文字距离场。
    fn layer_bind(&self, view: &wgpu::TextureView, text: &wgpu::TextureView) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("layer"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(text),
                },
            ],
        })
    }

    /// 这段字的距离场纹理（第一次用时算好上传；算不出来时 `None`）。
    fn text_field(&mut self, text: &ve_core::MaskText) -> Option<wgpu::TextureView> {
        if let Some(v) = self.text_fields.get(text) {
            return v.clone();
        }
        if self.text_fields.len() >= 32 {
            self.text_fields.clear();
        }
        let view = crate::text_mask::rasterize(&text.text, &text.font).map(|f| {
            let size = extent((f.width, f.height));
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("text-mask"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &f.data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(f.width),
                    rows_per_image: Some(f.height),
                },
                size,
            );
            texture.create_view(&wgpu::TextureViewDescriptor::default())
        });
        self.text_fields.insert(text.clone(), view.clone());
        view
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
        let bind = self.layer_bind(&view, &self.no_text);
        Slot {
            texture,
            view,
            size,
            bind,
            text: None,
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
        let backdrop = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("backdrop"),
            size: extent(size),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let backdrop_bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("backdrop"),
            layout: &self.backdrop_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&backdrop_view),
            }],
        });
        self.target = Some(Target {
            size,
            texture,
            readback,
            padded_row,
            backdrop,
            backdrop_bind,
            snapshots: Vec::new(),
        });
        self.nv12_target = None;
    }

    /// 上传各层像素和参数（槽位按尺寸复用）。
    #[expect(
        clippy::too_many_lines,
        reason = "每层的几何、颜色、外观、查找表参数一起打包，拆开反而难对照着色器"
    )]
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
        let short = to_f32(cw.min(ch));
        let mut instances = Vec::with_capacity(layers.len());
        let mut extras = Vec::with_capacity(layers.len());
        self.blended.clear();
        self.layer_luts.clear();
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
            let want = layer.mask_text.cloned();
            if self.slots[i].text != want {
                let field = want.as_ref().and_then(|t| self.text_field(t));
                let text = field.as_ref().unwrap_or(&self.no_text);
                self.slots[i].bind = self.layer_bind(&self.slots[i].view, text);
                self.slots[i].text = want;
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
            let uv = layer.geometry.uv;
            let [red, green, blue] = layer.color.0;
            // 四个角已经按旋转算好（左上、右上、右下、左下），这里只换成 NDC
            let ndc = |(x, y): (f32, f32)| {
                (
                    to_f32(f64::from(x) / cw * 2.0 - 1.0),
                    to_f32(1.0 - f64::from(y) / ch * 2.0),
                )
            };
            let [tl, tr, br, bl] = layer.geometry.corners().map(ndc);
            let shown = layer.geometry.shown;
            let lp = look_params(&layer.look, (shown.w, shown.h), short, layer.geometry.angle);
            #[expect(clippy::cast_precision_loss, reason = "混合模式编号很小")]
            let blend = layer.look.blend.code() as f32;
            self.blended
                .push(layer.look.blend != ve_core::BlendMode::Normal);
            // 进阶调色：整条链烘焙成查找表（同样的链只烘一次）
            let lut = (!layer.grades.is_empty()).then(|| chain_key(layer.grades));
            if let Some(key) = lut
                && !self.luts.contains_key(&key)
            {
                if self.luts.len() >= LUT_CACHE {
                    self.luts.clear();
                }
                let n = u32::try_from(ve_core::grade::BAKE_SIZE).unwrap_or(33);
                let table = ve_core::grade::bake(layer.grades);
                let bind = lut_bind(
                    &self.device,
                    &self.queue,
                    &self.lut_layout,
                    &self.lut_sampler,
                    n,
                    &table,
                );
                self.luts.insert(key, bind);
            }
            self.layer_luts.push(lut);
            extras.push(extra_masks(&layer.look, shown.w, shown.h));
            instances.push(LayerInstance {
                top: [tl.0, tl.1, tr.0, tr.1],
                bottom: [bl.0, bl.1, br.0, br.1],
                uv: [uv.x, uv.y, uv.x + uv.w, uv.y + uv.h],
                p0: [
                    layer.opacity.clamp(0.0, 1.0),
                    // 用查找表时矩阵已经在链里了
                    if layer.color.is_identity() || lut.is_some() {
                        0.0
                    } else {
                        1.0
                    },
                    blend,
                    lp.radius,
                ],
                red,
                green,
                blue,
                sizes: lp.sizes,
                stroke: lp.stroke,
                shadow: lp.shadow,
                shadow2: lp.shadow2,
                mask: lp.mask,
                mask2: lp.mask2,
                key: lp.key,
                key2: [
                    lp.key2[0],
                    lp.key2[1],
                    if lut.is_some() { 1.0 } else { 0.0 },
                    0.0,
                ],
                detail: [
                    layer.detail.sharpen,
                    layer.detail.grain,
                    layer.detail.vignette,
                    layer.detail.seed,
                ],
            });
        }
        // 画面特效：每道一个铺满画布的四边形，参数是（种类、强度、秒数）
        for e in &self.effects {
            extras.push(ExtraMasks::zeroed());
            instances.push(LayerInstance {
                top: [-1.0, 1.0, 1.0, 1.0],
                bottom: [-1.0, -1.0, 1.0, -1.0],
                p0: [e.effect.kind.code(), e.effect.amount, e.seconds, 0.0],
                sizes: [to_f32(cw), to_f32(ch), 0.0, 0.0],
                ..LayerInstance::zeroed()
            });
        }
        self.write_instances(&instances);
        self.write_extras(&extras);
        Ok(())
    }

    /// 写各层另外几个蒙版；容量不够时按 2 的幂重建缓冲和绑定组（至少一条，空缓冲不能绑）。
    fn write_extras(&mut self, extras: &[ExtraMasks]) {
        let need = extras.len().max(1);
        if self.extras.as_ref().is_none_or(|(_, _, cap)| *cap < need) {
            let cap = need.next_power_of_two();
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("extra-masks"),
                size: (cap * size_of::<ExtraMasks>()) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("extra-masks"),
                layout: &self.extras_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
            self.extras = Some((buffer, bind, cap));
        }
        if let Some((buffer, ..)) = &self.extras
            && !extras.is_empty()
        {
            self.queue
                .write_buffer(buffer, 0, bytemuck::cast_slice(extras));
        }
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

    /// 画面特效：之后合成的帧都按它做，直到再设。[`ve_core::EffectPass::after`] 是合成时传入的
    /// 图层下标（画完前这么多层以后做这道特效），超过层数的当作画完所有层以后。
    pub fn set_effects(&mut self, effects: &[ve_core::EffectPass]) {
        self.effects.clear();
        self.effects.extend_from_slice(effects);
    }

    /// 半透明复合片段的分组（[`ve_core::compose::GroupPass`]）：之后合成的帧都按它做，直到再设。
    pub fn set_groups(&mut self, groups: &[ve_core::compose::GroupPass]) {
        self.groups.clear();
        self.groups.extend_from_slice(groups);
        self.groups
            .sort_by_key(|g| (g.start, std::cmp::Reverse(g.end)));
    }

    /// 每组一张快照纹理（和输出一样大）、一个不透明度绑定。
    fn prepare_groups(&mut self) {
        let Some(target) = self.target.as_mut() else {
            return;
        };
        while target.snapshots.len() < self.groups.len() {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("group snapshot"),
                size: extent(target.size),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("group snapshot"),
                layout: &self.backdrop_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                }],
            });
            target.snapshots.push((texture, bind));
        }
        self.group_binds = self
            .groups
            .iter()
            .map(|g| {
                let v = group_params(g, target.size);
                let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("group amount"),
                    size: size_of_val(&v) as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.queue
                    .write_buffer(&buffer, 0, bytemuck::cast_slice(&v));
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("group amount"),
                    layout: &self.group_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    }],
                })
            })
            .collect();
    }

    /// 底色：黑色经 `color` 调色后的颜色（调节片段作用在没有画面的地方）；之后合成的帧都用它，
    /// 直到再设。
    pub fn set_background(&mut self, color: &ve_core::ColorMatrix) {
        let [r, g, b] = color.apply([0.0, 0.0, 0.0]).map(f64::from);
        self.background = wgpu::Color { r, g, b, a: 1.0 };
    }

    /// 底色：黑色经调色链调色后的颜色（调节片段用了进阶调色时）；链是空的时按 `color` 的矩阵。
    pub fn set_background_chain(
        &mut self,
        color: &ve_core::ColorMatrix,
        chain: &[ve_core::GradeStep],
    ) {
        if chain.is_empty() {
            self.set_background(color);
            return;
        }
        let [r, g, b] = ve_core::grade::apply_chain(chain, [0.0, 0.0, 0.0]).map(f64::from);
        self.background = wgpu::Color { r, g, b, a: 1.0 };
    }

    /// 第 `i` 层的查找表绑定组。
    fn lut_for(&self, i: usize) -> &wgpu::BindGroup {
        self.layer_luts
            .get(i)
            .copied()
            .flatten()
            .and_then(|k| self.luts.get(&k))
            .unwrap_or(&self.no_lut)
    }

    /// 开一个画到输出纹理上的渲染通道：第一次清成底色，之后接着画。
    fn begin_pass<'e>(
        &self,
        encoder: &'e mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        clear: bool,
    ) -> wgpu::RenderPass<'e> {
        let load = if clear {
            wgpu::LoadOp::Clear(self.background)
        } else {
            wgpu::LoadOp::Load
        };
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("compose"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        })
    }

    /// 把各层画到输出纹理上（底色见 [`Self::set_background`]）。普通图层连着画在一个通道里；
    /// 混合模式图层先把画到这里的结果拷成底图，再单独画它（读底图、直接写回）；
    /// 画面特效在它压着的那几层画完后做：同样拷成底图，再整个画面重画。
    /// 半透明复合片段（分组）：开始前拍一张快照，组里画完后和快照按整组不透明度混合。
    fn draw(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, count: usize) {
        let (Some((instances, _)), Some(target)) = (&self.instances, &self.target) else {
            drop(self.begin_pass(encoder, view, true));
            return;
        };
        // 第 k 道特效在画完前 after 层后做（超出层数的放最后）
        let effect_at = |k: usize| self.effects[k].after.min(count);
        let groups = &self.groups;
        let mut first = true;
        let mut next_effect = 0;
        let mut next_group = 0;
        // 还没合上的组（从外到里）
        let mut open: Vec<usize> = Vec::new();
        let mut start = 0;
        loop {
            // 1. 在这里结束的组（从里到外）：先做组里的特效，再和快照混合
            while let Some(&gi) = open.last() {
                let g = &groups[gi];
                if g.end.min(count) > start {
                    break;
                }
                while next_effect < g.effects_end.min(self.effects.len())
                    && effect_at(next_effect) <= start
                {
                    self.effect_pass(encoder, view, target, instances, count, next_effect);
                    next_effect += 1;
                }
                self.group_pass(encoder, view, target, gi);
                open.pop();
            }
            // 2. 压在前 start 层上的特效
            while next_effect < self.effects.len() && effect_at(next_effect) <= start {
                if first {
                    drop(self.begin_pass(encoder, view, true));
                    first = false;
                }
                self.effect_pass(encoder, view, target, instances, count, next_effect);
                next_effect += 1;
            }
            // 3. 从这里开始的组（从外到里）：拍快照
            while next_group < groups.len() && groups[next_group].start <= start {
                if first {
                    drop(self.begin_pass(encoder, view, true));
                    first = false;
                }
                encoder.copy_texture_to_texture(
                    target.texture.as_image_copy(),
                    target.snapshots[next_group].0.as_image_copy(),
                    extent(target.size),
                );
                open.push(next_group);
                next_group += 1;
            }
            if start >= count {
                break;
            }
            if self.blended.get(start).copied().unwrap_or(false) {
                if first {
                    drop(self.begin_pass(encoder, view, true));
                    first = false;
                }
                self.blend_layer_pass(encoder, view, target, instances, start);
                start += 1;
                continue;
            }
            // 一段连续的普通图层：到下一个混合模式图层、下一道特效、下一个组的开头或结尾为止
            let mut stop = if next_effect < self.effects.len() {
                effect_at(next_effect)
            } else {
                count
            };
            if let Some(g) = groups.get(next_group) {
                stop = stop.min(g.start);
            }
            if let Some(&gi) = open.last() {
                stop = stop.min(groups[gi].end);
            }
            let end = (start..stop)
                .find(|&i| self.blended.get(i).copied().unwrap_or(false))
                .unwrap_or(stop)
                .max(start + 1)
                .min(count);
            let mut pass = self.begin_pass(encoder, view, first);
            first = false;
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, instances.slice(..));
            pass.set_bind_group(1, &target.backdrop_bind, &[]);
            if let Some((_, extras, _)) = &self.extras {
                pass.set_bind_group(3, extras, &[]);
            }
            for i in start..end {
                let n = u32::try_from(i).unwrap_or(u32::MAX);
                pass.set_bind_group(0, &self.slots[i].bind, &[]);
                pass.set_bind_group(2, self.lut_for(i), &[]);
                pass.draw(0..6, n..n + 1);
            }
            drop(pass);
            start = end;
        }
        if first {
            drop(self.begin_pass(encoder, view, true));
        }
    }

    /// 第 `i` 层是混合模式图层：读底图、算好后直接写回。
    fn blend_layer_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        target: &Target,
        instances: &wgpu::Buffer,
        i: usize,
    ) {
        let n = u32::try_from(i).unwrap_or(u32::MAX);
        self.backdrop_pass(
            encoder,
            view,
            target,
            instances,
            &self.slots[i].bind,
            n,
            &self.blend_pipeline,
            false,
        );
    }

    /// 第 `k` 道画面特效（插件没登记或编译不过时跳过）。
    fn effect_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        target: &Target,
        instances: &wgpu::Buffer,
        count: usize,
        k: usize,
    ) {
        let n = u32::try_from(count + k).unwrap_or(u32::MAX);
        let e = self.effects[k].effect;
        let plugin = (e.kind == ve_core::EffectKind::Plugin)
            .then(|| self.plugin_pipeline(e.plugin.as_str()));
        match plugin {
            Some(None) => {}
            Some(Some(p)) => self.backdrop_pass(
                encoder,
                view,
                target,
                instances,
                &self.dummy_layer,
                n,
                &p,
                true,
            ),
            None => self.backdrop_pass(
                encoder,
                view,
                target,
                instances,
                &self.dummy_layer,
                n,
                &self.effect_pipeline,
                true,
            ),
        }
    }

    /// 第 `gi` 组画完：把画好的拷成底图，和组开始前的快照按整组不透明度混合后写回。
    fn group_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        target: &Target,
        gi: usize,
    ) {
        encoder.copy_texture_to_texture(
            target.texture.as_image_copy(),
            target.backdrop.as_image_copy(),
            extent(target.size),
        );
        let mut pass = self.begin_pass(encoder, view, false);
        pass.set_pipeline(&self.group_pipeline);
        pass.set_bind_group(0, &target.snapshots[gi].1, &[]);
        pass.set_bind_group(1, &target.backdrop_bind, &[]);
        pass.set_bind_group(2, &self.group_binds[gi], &[]);
        pass.draw(0..3, 0..1);
    }

    /// 插件 `name` 的管线：第一次用到时编译（拼上内置着色器）；没登记、编译失败返回 `None`（跳过这道）。
    fn plugin_pipeline(&self, name: &str) -> Option<wgpu::RenderPipeline> {
        use std::hash::{Hash as _, Hasher as _};
        let source = crate::plugin::source(name)?;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut h);
        let key = (name.to_owned(), h.finish());
        let mut cache = self
            .plugins
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache
            .entry(key)
            .or_insert_with(|| {
                // 登记时已经检查过；这里再兜一层，编译出错不让整个程序崩。naga 检查通过、
                // 显卡后端（DX12 的着色器编译器）编不过的算内部错误，也要接住
                let internal = self.device.push_error_scope(wgpu::ErrorFilter::Internal);
                let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
                let layouts = [
                    &self.layout,
                    &self.backdrop_layout,
                    &self.lut_layout,
                    &self.extras_layout,
                ];
                let full = crate::plugin::full_source(&source);
                let pipeline = pipeline_from(
                    &self.device,
                    layouts,
                    &full,
                    crate::plugin::ENTRY_POINT,
                    false,
                );
                let failed = pollster::block_on(scope.pop());
                let failed = failed.or_else(|| pollster::block_on(internal.pop()));
                failed.map_or(Some(pipeline), |e| {
                    log::warn!("插件特效 {name} 编译失败：{e}");
                    None
                })
            })
            .clone()
    }

    /// 读底图的一道：把画到这里的结果拷成底图，再画第 `n` 个实例（混合模式图层或画面特效）。
    #[expect(clippy::too_many_arguments, reason = "绘制参数，拆成结构体反而难读")]
    fn backdrop_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        target: &Target,
        instances: &wgpu::Buffer,
        layer: &wgpu::BindGroup,
        n: u32,
        pipeline: &wgpu::RenderPipeline,
        effect: bool,
    ) {
        encoder.copy_texture_to_texture(
            target.texture.as_image_copy(),
            target.backdrop.as_image_copy(),
            extent(target.size),
        );
        let mut pass = self.begin_pass(encoder, view, false);
        pass.set_pipeline(pipeline);
        pass.set_vertex_buffer(0, instances.slice(..));
        pass.set_bind_group(0, layer, &[]);
        let lut = if effect {
            &self.no_lut
        } else {
            self.lut_for(n as usize)
        };
        pass.set_bind_group(2, lut, &[]);
        pass.set_bind_group(1, &target.backdrop_bind, &[]);
        if let Some((_, extras, _)) = &self.extras {
            pass.set_bind_group(3, extras, &[]);
        }
        pass.draw(0..6, n..n + 1);
    }

    /// 上传并画到输出纹理，返回编码器（还没提交）。
    fn render(
        &mut self,
        size: (u32, u32),
        layers: &[Layer<'_>],
    ) -> Result<wgpu::CommandEncoder, RenderError> {
        self.upload(size, layers)?;
        self.ensure_target(size);
        self.prepare_groups();
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
                timeout: Some(GPU_WAIT),
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
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(GPU_WAIT),
        })
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
            center: (w.mul_add(0.5, x), h.mul_add(0.5, y)),
            angle: 0.0,
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
            color: ve_core::ColorMatrix::IDENTITY,
            look: ve_core::Look::NONE,
            grades: &[],
            detail: ve_core::Detail::default(),
            mask_text: None,
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
        center: (32.0, 18.0),
        angle: 0.0,
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
    fn rotated_layer_turns_about_its_center() {
        let mut c = compositor();
        let red = solid(4, 4, [0, 0, 255, 255]);
        // 32×8 的横条，中心 (32, 18)；顺时针转 90° 变成竖条：x 28–36、y 2–34
        let g = LayerGeometry {
            angle: std::f32::consts::FRAC_PI_2,
            ..geom(16.0, 14.0, 32.0, 8.0)
        };
        let mut out = Vec::new();
        c.compose_bgra(SIZE, &[layer(&red, 4, 4, g, 1.0)], &mut out)
            .unwrap();
        assert!(
            near(px(&out, 32, 4), [0, 0, 255], 0),
            "{:?}",
            px(&out, 32, 4)
        );
        assert!(near(px(&out, 32, 31), [0, 0, 255], 0));
        // 没转时横条覆盖的地方，现在是黑的
        assert!(near(px(&out, 18, 18), [0, 0, 0], 0));
        assert!(near(px(&out, 45, 18), [0, 0, 0], 0));
    }

    fn with_look(mut l: Layer<'_>, look: ve_core::Look) -> Layer<'_> {
        l.look = look;
        l
    }

    fn compose(c: &mut Compositor, layers: &[Layer<'_>]) -> Vec<u8> {
        let mut out = Vec::new();
        c.compose_bgra(SIZE, layers, &mut out).unwrap();
        out
    }

    #[test]
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::shadow_unrelated,
        reason = "测试：0–255 的期望值"
    )]
    fn blend_modes_match_the_formulas() {
        use ve_core::BlendMode;
        let mut c = compositor();
        let base = solid(2, 2, [128, 128, 128, 255]);
        let top = solid(2, 2, [64, 128, 192, 255]);
        for mode in BlendMode::ALL {
            let look = ve_core::Look {
                blend: mode,
                ..ve_core::Look::NONE
            };
            // 底下一层普通的、上面再叠一层同样模式的：两层混合模式要能连着用
            let out = compose(
                &mut c,
                &[
                    layer(&base, 2, 2, FULL, 1.0),
                    with_look(layer(&top, 2, 2, FULL, 1.0), look),
                ],
            );
            let want = [64u8, 128, 192].map(|s| {
                let v = mode.channel(128.0 / 255.0, f32::from(s) / 255.0);
                (v * 255.0).round() as u8
            });
            assert!(
                near(px(&out, 32, 18), want, 2),
                "{mode:?}: {:?} / {want:?}",
                px(&out, 32, 18)
            );
        }
        // 半透明：按不透明度在底色和混合结果之间
        let multiply = ve_core::Look {
            blend: BlendMode::Multiply,
            ..ve_core::Look::NONE
        };
        let out = compose(
            &mut c,
            &[
                layer(&base, 2, 2, FULL, 1.0),
                with_look(layer(&top, 2, 2, FULL, 0.5), multiply),
            ],
        );
        let want = [64.0f32, 128.0, 192.0].map(|s| {
            let b = 128.0 / 255.0;
            ((b * 0.5 + b * s / 255.0 * 0.5) * 255.0).round() as u8
        });
        assert!(near(px(&out, 32, 18), want, 2), "{:?}", px(&out, 32, 18));
        // 两个混合模式图层叠在一起：第二个读到的底图里已经有第一个
        let out = compose(
            &mut c,
            &[
                layer(&base, 2, 2, FULL, 1.0),
                with_look(layer(&top, 2, 2, FULL, 1.0), multiply),
                with_look(layer(&top, 2, 2, FULL, 1.0), multiply),
            ],
        );
        let want = [64.0f32, 128.0, 192.0].map(|s| {
            let s = s / 255.0;
            (128.0 / 255.0 * s * s * 255.0).round() as u8
        });
        assert!(near(px(&out, 32, 18), want, 2), "{:?}", px(&out, 32, 18));
    }

    #[test]
    fn round_corners_cut_the_corners() {
        let mut c = compositor();
        let white = solid(2, 2, [255; 4]);
        let look = ve_core::Look {
            radius: 1.0,
            ..ve_core::Look::NONE
        };
        let out = compose(&mut c, &[with_look(layer(&white, 2, 2, FULL, 1.0), look)]);
        assert!(near(px(&out, 0, 0), [0, 0, 0], 1), "角上透出底色");
        assert!(near(px(&out, 32, 18), [255, 255, 255], 1));
        assert!(near(px(&out, 32, 1), [255, 255, 255], 1), "上边中间还在");
    }

    #[test]
    #[expect(clippy::shadow_unrelated, reason = "测试：一次一次合成")]
    fn stroke_rings_the_frame_and_shadow_falls_behind() {
        let mut c = compositor();
        let white = solid(2, 2, [255; 4]);
        let gray = solid(2, 2, [100, 100, 100, 255]);
        let inner = geom(16.0, 8.0, 32.0, 20.0);
        let stroke = ve_core::Look {
            stroke: Some(ve_core::Stroke {
                color: 0xFF_0000,
                width: 0.1,
            }),
            ..ve_core::Look::NONE
        };
        let out = compose(
            &mut c,
            &[with_look(layer(&white, 2, 2, inner, 1.0), stroke)],
        );
        // 描边 3.6 像素：左边外面 2 像素是红的，再往外是黑的，里面还是白的
        assert!(
            near(px(&out, 14, 18), [0, 0, 255], 2),
            "{:?}",
            px(&out, 14, 18)
        );
        assert!(near(px(&out, 8, 18), [0, 0, 0], 1));
        assert!(near(px(&out, 20, 18), [255, 255, 255], 1));
        // 阴影往右 3.6 像素、不模糊：画面右边外面变黑，左边外面还是灰的
        let shadow = ve_core::Look {
            shadow: Some(ve_core::Shadow {
                color: 0,
                opacity: 1.0,
                blur: 0.0,
                distance: 0.1,
                angle: 0.0,
            }),
            ..ve_core::Look::NONE
        };
        let out = compose(
            &mut c,
            &[
                layer(&gray, 2, 2, FULL, 1.0),
                with_look(layer(&white, 2, 2, inner, 1.0), shadow),
            ],
        );
        assert!(
            near(px(&out, 50, 18), [0, 0, 0], 2),
            "{:?}",
            px(&out, 50, 18)
        );
        assert!(near(px(&out, 14, 18), [100, 100, 100], 2));
        assert!(
            near(px(&out, 30, 18), [255, 255, 255], 1),
            "画面压在影子上面"
        );
    }

    #[test]
    fn a_text_mask_keeps_only_the_letters() {
        use ve_core::{Mask, MaskShape, MaskText};
        let mut c = compositor();
        let white = solid(2, 2, [255; 4]);
        let mask = Mask {
            width: 0.95,
            height: 0.95,
            ..Mask::new(MaskShape::Text)
        };
        let words = MaskText {
            text: "I".to_owned(),
            font: String::new(),
        };
        let masked = |m: Mask| {
            let mut l = with_look(
                layer(&white, 2, 2, FULL, 1.0),
                ve_core::Look {
                    mask: Some(m),
                    ..ve_core::Look::NONE
                },
            );
            l.mask_text = Some(&words);
            l
        };
        let out = compose(&mut c, &[masked(mask)]);
        // 「I」是中间一竖：中间那一行有亮的，两边和四角都去掉
        let row: Vec<u8> = (0..SIZE.0).map(|x| px(&out, x, SIZE.1 / 2)[0]).collect();
        let lit: Vec<u32> = (0..SIZE.0).filter(|&x| row[x as usize] > 200).collect();
        assert!(!lit.is_empty(), "{row:?}");
        assert!(
            lit.iter().all(|&x| x.abs_diff(SIZE.0 / 2) <= 4),
            "只有中间亮：{lit:?}"
        );
        assert!(near(px(&out, 1, 1), [0, 0, 0], 2));
        assert!(near(px(&out, 10, SIZE.1 / 2), [0, 0, 0], 2));
        // 反转：字去掉、别处留下
        let inverted = compose(
            &mut c,
            &[masked(Mask {
                invert: true,
                ..mask
            })],
        );
        assert!(near(px(&inverted, 10, SIZE.1 / 2), [255, 255, 255], 2));
        let dark = lit.iter().all(|&x| px(&inverted, x, SIZE.1 / 2)[0] < 60);
        assert!(dark, "字的地方去掉了");
        // 没有字时（文字蒙版但片段上没写）也不出错：整层去掉
        let mut blank = masked(mask);
        blank.mask_text = None;
        let none = compose(&mut c, &[blank]);
        assert!(near(px(&none, SIZE.0 / 2, SIZE.1 / 2), [0, 0, 0], 2));
    }

    #[test]
    #[expect(clippy::shadow_unrelated, reason = "测试：一次一次合成")]
    fn masks_keep_their_shape() {
        use ve_core::{Mask, MaskShape};
        let mut c = compositor();
        let white = solid(2, 2, [255; 4]);
        let look = |m: Mask| ve_core::Look {
            mask: Some(m),
            ..ve_core::Look::NONE
        };
        for shape in MaskShape::ALL.into_iter().filter(|s| *s != MaskShape::Text) {
            let m = Mask::new(shape);
            let out = compose(
                &mut c,
                &[with_look(layer(&white, 2, 2, FULL, 1.0), look(m))],
            );
            let center = if shape == MaskShape::Linear {
                // 线性：留上面
                px(&out, 32, 6)
            } else {
                px(&out, 32, 18)
            };
            assert!(
                near(center, [255, 255, 255], 2),
                "{shape:?} 中间留着：{center:?}"
            );
            assert!(near(px(&out, 1, 34), [0, 0, 0], 2), "{shape:?} 左下角去掉");
            let inv = compose(
                &mut c,
                &[with_look(
                    layer(&white, 2, 2, FULL, 1.0),
                    look(Mask { invert: true, ..m }),
                )],
            );
            assert!(near(px(&inv, 1, 34), [255, 255, 255], 2), "{shape:?} 反转");
        }
        // 镜面：上下边缘去掉，中间一条留着；转 90° 后变成竖的
        let mirror = Mask::new(MaskShape::Mirror);
        let out = compose(
            &mut c,
            &[with_look(layer(&white, 2, 2, FULL, 1.0), look(mirror))],
        );
        assert!(near(px(&out, 5, 18), [255, 255, 255], 2));
        assert!(near(px(&out, 32, 2), [0, 0, 0], 2));
        // 羽化：边上是灰的
        let soft = Mask {
            feather: 0.5,
            ..Mask::new(MaskShape::Circle)
        };
        let out = compose(
            &mut c,
            &[with_look(layer(&white, 2, 2, FULL, 1.0), look(soft))],
        );
        let edge = px(&out, 32 + 9, 18)[0];
        assert!(edge > 20 && edge < 235, "羽化后的边缘 {edge}");
    }

    #[test]
    fn advanced_grades_match_the_cpu_chain() {
        use std::sync::Arc;
        let mut c = compositor();
        // 几种颜色各一列：红、绿、蓝、灰、橙
        let colors: [[u8; 3]; 5] = [
            [220, 40, 40],
            [40, 200, 60],
            [30, 60, 230],
            [128, 128, 128],
            [240, 150, 30],
        ];
        let mut data = Vec::new();
        for _ in 0..4 {
            for rgb in colors {
                data.extend([rgb[2], rgb[1], rgb[0], 255]);
            }
        }
        let mut g = ve_core::Grade::default();
        g.hsl[ve_core::HslBand::Red.index()].saturation = -0.8;
        g.curves.luma = vec![(0.25, 0.15), (0.75, 0.9)];
        g.wheels.highlights = [0.2, 0.0, -0.3];
        let warm = ve_core::ColorAdjust {
            temperature: 0.3,
            contrast: 0.1,
            ..ve_core::ColorAdjust::NONE
        };
        let chain = vec![ve_core::GradeStep {
            tone: None,
            input: None,
            matrix: warm.matrix(),
            grade: Some(Arc::new(g)),
        }];
        let mut l = layer(&data, 5, 4, FULL, 1.0);
        l.grades = &chain;
        let out = compose(&mut c, &[l]);
        for (i, col) in colors.iter().enumerate() {
            // 每一列的正中间（拉伸后线性过滤不混进旁边那一列）
            let x = (2 * u32::try_from(i).unwrap() + 1) * 32 / 5;
            let want = ve_core::grade::apply_chain(&chain, col.map(|v| f32::from(v) / 255.0));
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "0–1 的颜色"
            )]
            let want = [want[2], want[1], want[0]].map(|v| (v * 255.0).round() as u8);
            // 33³ 的查找表（LUT 的通用尺寸）在中性色附近有几个色阶的插值误差：
            // 灰的邻格是偏红的颜色，会被「红色降饱和」大幅改动
            assert!(
                near(px(&out, x, 18), want, 8),
                "第 {i} 列：{:?} / {want:?}",
                px(&out, x, 18)
            );
        }
    }

    #[test]
    fn several_masks_show_what_any_of_them_keeps() {
        use ve_core::{Mask, MaskShape};
        let mut c = compositor();
        let white = solid(2, 2, [255; 4]);
        let rect = |x: f32| Mask {
            x,
            width: 0.3,
            height: 1.0,
            ..Mask::new(MaskShape::Rectangle)
        };
        let look = ve_core::Look {
            mask: Some(rect(-0.3)),
            extra_masks: [Some(rect(0.3)), None, None],
            ..ve_core::Look::NONE
        };
        let out = compose(&mut c, &[with_look(layer(&white, 2, 2, FULL, 1.0), look)]);
        assert!(near(px(&out, 13, 18), [255, 255, 255], 2), "左边蒙版留下的");
        assert!(near(px(&out, 51, 18), [255, 255, 255], 2), "右边蒙版留下的");
        assert!(near(px(&out, 32, 18), [0, 0, 0], 2), "两个都没留的中间");
        // 第二个蒙版反转：除了它的那一块，别的地方都留下
        let inv = ve_core::Look {
            extra_masks: [
                Some(Mask {
                    invert: true,
                    ..rect(0.3)
                }),
                None,
                None,
            ],
            ..look
        };
        let inverted = compose(&mut c, &[with_look(layer(&white, 2, 2, FULL, 1.0), inv)]);
        assert!(near(px(&inverted, 32, 18), [255, 255, 255], 2));
        assert!(
            near(px(&inverted, 51, 18), [0, 0, 0], 2),
            "只有两个都不留的地方没了"
        );
    }

    #[test]
    fn vignette_grain_and_sharpen_run_in_the_shader() {
        let mut c = compositor();
        let gray = solid(64, 36, [128, 128, 128, 255]);
        // 暗角：四角暗、中间不变
        let mut vig = layer(&gray, 64, 36, FULL, 1.0);
        vig.detail = ve_core::Detail {
            vignette: 1.0,
            ..ve_core::Detail::default()
        };
        let out = compose(&mut c, &[vig]);
        assert!(
            near(px(&out, 32, 18), [128, 128, 128], 2),
            "{:?}",
            px(&out, 32, 18)
        );
        assert!(px(&out, 0, 0)[0] < 60, "{:?}", px(&out, 0, 0));
        // 颗粒：平的灰变得有起伏，换个种子图案就变
        let mut grainy = layer(&gray, 64, 36, FULL, 1.0);
        grainy.detail = ve_core::Detail {
            grain: 1.0,
            ..ve_core::Detail::default()
        };
        let first = compose(&mut c, &[grainy]);
        let values: std::collections::HashSet<u8> = first.chunks(4).map(|p| p[0]).collect();
        assert!(values.len() > 5, "{values:?}");
        let mut reseeded = layer(&gray, 64, 36, FULL, 1.0);
        reseeded.detail = ve_core::Detail {
            grain: 1.0,
            seed: 7.0,
            ..ve_core::Detail::default()
        };
        assert_ne!(compose(&mut c, &[reseeded]), first);
        // 锐化：左黑右白的竖边两边拉开
        let mut edge = Vec::new();
        for _ in 0..36 {
            for x in 0..64 {
                let v = if x < 32 { 64 } else { 192 };
                edge.extend([v, v, v, 255]);
            }
        }
        let plain = compose(&mut c, &[layer(&edge, 64, 36, FULL, 1.0)]);
        let mut sharpened = layer(&edge, 64, 36, FULL, 1.0);
        sharpened.detail = ve_core::Detail {
            sharpen: 1.0,
            ..ve_core::Detail::default()
        };
        let sharp = compose(&mut c, &[sharpened]);
        assert!(
            px(&sharp, 31, 18)[0] < px(&plain, 31, 18)[0],
            "暗的一边更暗"
        );
        assert!(
            px(&sharp, 32, 18)[0] > px(&plain, 32, 18)[0],
            "亮的一边更亮"
        );
        assert!(near(px(&sharp, 10, 18), [64, 64, 64], 1), "平的地方不变");
    }

    #[test]
    fn tone_items_match_the_cpu_chain() {
        let mut c = compositor();
        let data = solid(64, 36, [40, 60, 80, 255]);
        let tone = ve_core::ColorAdjust {
            shadows: 0.8,
            light: 0.5,
            ..ve_core::ColorAdjust::NONE
        };
        let chain = vec![ve_core::GradeStep {
            input: None,
            matrix: tone.matrix(),
            tone: tone.tone(),
            grade: None,
        }];
        let mut l = layer(&data, 64, 36, FULL, 1.0);
        l.grades = &chain;
        let out = compose(&mut c, &[l]);
        let want = ve_core::grade::apply_chain(&chain, [80.0 / 255.0, 60.0 / 255.0, 40.0 / 255.0]);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "0–1 的颜色"
        )]
        let want = [want[2], want[1], want[0]].map(|v| (v * 255.0).round() as u8);
        assert!(
            near(px(&out, 32, 18), want, 3),
            "{:?} / {want:?}",
            px(&out, 32, 18)
        );
        assert!(px(&out, 32, 18)[2] > 90, "阴影、光感提亮了");
    }

    #[test]
    fn background_follows_the_adjustment_chain() {
        use std::sync::Arc;
        let mut c = compositor();
        let mut g = ve_core::Grade::default();
        g.wheels.offset = [0.4, 0.0, 0.0];
        let chain = vec![ve_core::GradeStep {
            tone: None,
            input: None,
            matrix: ve_core::ColorMatrix::IDENTITY,
            grade: Some(Arc::new(g)),
        }];
        c.set_background_chain(&ve_core::ColorMatrix::IDENTITY, &chain);
        let out = compose(&mut c, &[]);
        assert!(
            near(px(&out, 1, 1), [0, 0, 26], 2),
            "黑底偏红：{:?}",
            px(&out, 1, 1)
        );
    }

    #[test]
    fn stroke_follows_the_mask_shape() {
        use ve_core::{Mask, MaskShape};
        let mut c = compositor();
        let white = solid(2, 2, [255; 4]);
        // 圆形蒙版（直径 = 画面高的一半 = 18 像素）+ 红色描边 2 像素
        let look = ve_core::Look {
            mask: Some(Mask::new(MaskShape::Circle)),
            stroke: Some(ve_core::Stroke {
                color: 0xFF_0000,
                width: 2.0 / 36.0,
            }),
            ..ve_core::Look::NONE
        };
        let out = compose(&mut c, &[with_look(layer(&white, 2, 2, FULL, 1.0), look)]);
        // 圆的上边在 y = 18 - 9 = 9：上面 1 像素是描边
        assert!(
            near(px(&out, 32, 8), [0, 0, 255], 40),
            "{:?}",
            px(&out, 32, 8)
        );
        assert!(near(px(&out, 32, 18), [255, 255, 255], 2));
        // 画面框的边上不再有描边
        assert!(near(px(&out, 32, 0), [0, 0, 0], 2), "{:?}", px(&out, 32, 0));
    }

    #[test]
    #[expect(clippy::shadow_unrelated, reason = "测试：一次一次合成")]
    fn chroma_key_removes_green_and_keeps_red() {
        let mut c = compositor();
        let green = solid(2, 2, [0, 255, 0, 255]);
        let red = solid(2, 2, [0, 0, 255, 255]);
        let gray = solid(2, 2, [100, 100, 100, 255]);
        let key = ve_core::Look {
            chroma: Some(ve_core::ChromaKey::default()),
            ..ve_core::Look::NONE
        };
        let out = compose(
            &mut c,
            &[
                layer(&gray, 2, 2, FULL, 1.0),
                with_look(layer(&green, 2, 2, FULL, 1.0), key),
            ],
        );
        assert!(
            near(px(&out, 32, 18), [100, 100, 100], 2),
            "绿色抠掉，露出下面"
        );
        let out = compose(
            &mut c,
            &[
                layer(&gray, 2, 2, FULL, 1.0),
                with_look(layer(&red, 2, 2, FULL, 1.0), key),
            ],
        );
        assert!(near(px(&out, 32, 18), [0, 0, 255], 2), "红色留着");
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
    fn color_adjust_matches_the_cpu_formula() {
        let mut comp = compositor();
        // 纯红（BGRA），饱和度拉到 -1、亮度 +0.4、色温 +0.5
        let red = solid(2, 2, [0, 0, 255, 255]);
        let adjust = ve_core::ColorAdjust {
            brightness: 0.4,
            saturation: -1.0,
            temperature: 0.5,
            ..ve_core::ColorAdjust::NONE
        };
        let mut tinted = layer(&red, 2, 2, FULL, 1.0);
        tinted.color = adjust.matrix();
        let mut out = Vec::new();
        comp.compose_bgra(SIZE, &[tinted], &mut out).unwrap();
        let [red_v, green_v, blue_v] = adjust.apply([1.0, 0.0, 0.0]);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "0–255"
        )]
        let want = [blue_v, green_v, red_v].map(|v| (v * 255.0).round() as u8);
        let got = px(&out, 32, 18);
        assert!(near(got, want, 2), "{got:?} vs {want:?}");
        // 不调色的图层原样（不经过反预乘，边缘半透明像素不变色）
        let plain = layer(&red, 2, 2, FULL, 1.0);
        comp.compose_bgra(SIZE, &[plain], &mut out).unwrap();
        assert_eq!(px(&out, 32, 18), [0, 0, 255, 255]);
    }

    #[test]
    fn adjusting_each_layer_and_the_background_equals_adjusting_the_whole_picture() {
        let mut comp = compositor();
        // 左半边：不透明的蓝上面压半透明的红；右半边什么都没有（黑底）
        let blue = solid(2, 2, [255, 0, 0, 255]);
        let red = solid(2, 2, [0, 0, 255, 255]);
        let mut half = FULL;
        half.full.w = 32.0;
        half.shown = half.full;
        half.center = (16.0, 18.0);
        let m = ve_core::ColorAdjust {
            brightness: 0.2,
            saturation: -1.0,
            ..ve_core::ColorAdjust::NONE
        }
        .matrix();
        let mut bottom = layer(&blue, 2, 2, half, 1.0);
        let mut top = layer(&red, 2, 2, half, 0.5);
        bottom.color = m;
        top.color = m;
        comp.set_background(&m);
        let mut out = Vec::new();
        comp.compose_bgra(SIZE, &[bottom, top], &mut out).unwrap();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "0–255"
        )]
        let bgr = |[r, g, b]: [f32; 3]| [b, g, r].map(|v| (v * 255.0).round() as u8);
        // 先叠好（红蓝各半）再整体调色
        let mixed = m.apply([0.5, 0.0, 0.5]);
        let got = px(&out, 8, 18);
        assert!(near(got, bgr(mixed), 2), "{got:?} vs {:?}", bgr(mixed));
        let bg = px(&out, 56, 18);
        let want_bg = bgr(m.apply([0.0, 0.0, 0.0]));
        assert!(near(bg, want_bg, 1), "底色 {bg:?} vs {want_bg:?}");
        // 换回黑底
        comp.set_background(&ve_core::ColorMatrix::IDENTITY);
        comp.compose_bgra(SIZE, &[], &mut out).unwrap();
        assert_eq!(px(&out, 56, 18), [0, 0, 0, 255]);
    }

    fn pass(
        after: usize,
        kind: ve_core::EffectKind,
        amount: f32,
        seconds: f32,
    ) -> ve_core::EffectPass {
        ve_core::EffectPass {
            after,
            effect: ve_core::Effect {
                kind,
                amount,
                plugin: ve_core::effect::PluginName::EMPTY,
                speed: ve_core::Effect::DEFAULT_SPEED,
            },
            seconds,
        }
    }

    /// 左半红、右半蓝的整幅图层。
    fn red_blue() -> Vec<u8> {
        let mut tex = Vec::new();
        for _ in 0..36 {
            tex.extend(solid(32, 1, [0, 0, 255, 255]));
            tex.extend(solid(32, 1, [255, 0, 0, 255]));
        }
        tex
    }

    #[test]
    fn a_translucent_group_fades_as_one_picture() {
        let mut c = compositor();
        let red = solid(2, 2, [0, 0, 255, 255]);
        let green = solid(2, 2, [0, 255, 0, 255]);
        let blue = solid(2, 2, [255, 0, 0, 255]);
        let mut out = Vec::new();
        let group = |start, end, opacity| ve_core::compose::GroupPass {
            start,
            end,
            effects_end: 0,
            opacity,
            shape: None,
        };
        // 红、绿完全重叠，整组 50%：黑底上一半的绿，红一点都透不出来
        c.set_groups(&[group(0, 2, 0.5)]);
        c.compose_bgra(
            SIZE,
            &[layer(&red, 2, 2, FULL, 1.0), layer(&green, 2, 2, FULL, 1.0)],
            &mut out,
        )
        .unwrap();
        assert!(
            near(px(&out, 20, 20), [0, 128, 0], 3),
            "{:?}",
            px(&out, 20, 20)
        );
        // 组下面还有一层蓝：蓝和绿各一半
        c.set_groups(&[group(1, 2, 0.5)]);
        c.compose_bgra(
            SIZE,
            &[
                layer(&blue, 2, 2, FULL, 1.0),
                layer(&green, 2, 2, FULL, 1.0),
            ],
            &mut out,
        )
        .unwrap();
        assert!(
            near(px(&out, 20, 20), [128, 128, 0], 3),
            "{:?}",
            px(&out, 20, 20)
        );
        // 嵌套：外层 50% 里面再套一层 50%
        c.set_groups(&[group(0, 1, 0.5), group(0, 1, 0.5)]);
        c.compose_bgra(SIZE, &[layer(&green, 2, 2, FULL, 1.0)], &mut out)
            .unwrap();
        assert!(
            near(px(&out, 20, 20), [0, 64, 0], 3),
            "{:?}",
            px(&out, 20, 20)
        );
        c.set_groups(&[]);
    }

    #[test]
    fn a_masked_group_shows_whats_below_outside_the_mask() {
        let mut c = compositor();
        let blue = solid(2, 2, [255, 0, 0, 255]);
        let green = solid(2, 2, [0, 255, 0, 255]);
        let mut out = Vec::new();
        let shaped =
            |look: ve_core::Look, transform: ve_core::Transform| ve_core::compose::GroupPass {
                start: 1,
                end: 2,
                effects_end: 0,
                opacity: 1.0,
                shape: Some(ve_core::compose::GroupShape { transform, look }),
            };
        let layers = [
            layer(&blue, 2, 2, FULL, 1.0),
            layer(&green, 2, 2, FULL, 1.0),
        ];
        // 矩形蒙版留左半边：左边是组里的绿，右边露出下面的蓝
        let mut left = ve_core::Mask::new(ve_core::MaskShape::Rectangle);
        (left.x, left.y, left.width, left.height) = (-0.25, 0.0, 0.5, 1.0);
        let look = ve_core::Look {
            mask: Some(left),
            ..ve_core::Look::NONE
        };
        c.set_groups(&[shaped(look, ve_core::Transform::DEFAULT)]);
        c.compose_bgra(SIZE, &layers, &mut out).unwrap();
        assert!(
            near(px(&out, 8, 18), [0, 255, 0], 3),
            "{:?}",
            px(&out, 8, 18)
        );
        assert!(
            near(px(&out, 56, 18), [255, 0, 0], 3),
            "{:?}",
            px(&out, 56, 18)
        );
        // 再加一个蒙版留最右边一条：两块都留（并集），中间偏右还是下面的蓝
        let mut right = left;
        (right.x, right.width) = (0.375, 0.25);
        let two = ve_core::Look {
            extra_masks: [Some(right), None, None],
            ..look
        };
        c.set_groups(&[shaped(two, ve_core::Transform::DEFAULT)]);
        c.compose_bgra(SIZE, &layers, &mut out).unwrap();
        assert!(
            near(px(&out, 60, 18), [0, 255, 0], 3),
            "{:?}",
            px(&out, 60, 18)
        );
        assert!(
            near(px(&out, 40, 18), [255, 0, 0], 3),
            "{:?}",
            px(&out, 40, 18)
        );
        // 整组往右挪半个画面：蒙版跟着走，留下的是画面中间那一条
        let moved = ve_core::Transform {
            x: 0.5,
            ..ve_core::Transform::DEFAULT
        };
        c.set_groups(&[shaped(look, moved)]);
        c.compose_bgra(SIZE, &layers, &mut out).unwrap();
        assert!(
            near(px(&out, 8, 18), [255, 0, 0], 3),
            "{:?}",
            px(&out, 8, 18)
        );
        assert!(
            near(px(&out, 40, 18), [0, 255, 0], 3),
            "{:?}",
            px(&out, 40, 18)
        );
        // 圆角（胶囊）：角上露出下面，中间是组里的
        let round = ve_core::Look {
            radius: 1.0,
            ..ve_core::Look::NONE
        };
        c.set_groups(&[shaped(round, ve_core::Transform::DEFAULT)]);
        c.compose_bgra(SIZE, &layers, &mut out).unwrap();
        assert!(near(px(&out, 1, 1), [255, 0, 0], 3), "{:?}", px(&out, 1, 1));
        assert!(
            near(px(&out, 32, 18), [0, 255, 0], 3),
            "{:?}",
            px(&out, 32, 18)
        );
        c.set_groups(&[]);
    }

    #[test]
    fn effects_apply_to_what_is_below_them() {
        use ve_core::EffectKind as K;
        let mut c = compositor();
        let white = solid(2, 2, [255, 255, 255, 255]);
        let mut out = Vec::new();
        // 暗角：中心不动、角落变黑
        c.set_effects(&[pass(1, K::Vignette, 1.0, 0.0)]);
        c.compose_bgra(SIZE, &[layer(&white, 2, 2, FULL, 1.0)], &mut out)
            .unwrap();
        assert!(near(px(&out, 32, 18), [255, 255, 255], 3));
        assert!(px(&out, 0, 0)[0] < 30, "{:?}", px(&out, 0, 0));
        // 闪白：片段开头整个变白、快到一秒时退回原样
        let blue = solid(2, 2, [255, 0, 0, 255]);
        c.set_effects(&[pass(1, K::Flash, 1.0, 0.0)]);
        c.compose_bgra(SIZE, &[layer(&blue, 2, 2, FULL, 1.0)], &mut out)
            .unwrap();
        assert!(near(px(&out, 10, 10), [255, 255, 255], 3));
        c.set_effects(&[pass(1, K::Flash, 1.0, 0.95)]);
        c.compose_bgra(SIZE, &[layer(&blue, 2, 2, FULL, 1.0)], &mut out)
            .unwrap();
        assert!(
            near(px(&out, 10, 10), [255, 0, 0], 3),
            "{:?}",
            px(&out, 10, 10)
        );
        // 特效只压在它下面：闪白在第 0 层之前（只作用于黑底），上面盖左半边的蓝不受影响
        let mut half = FULL;
        half.full.w = 32.0;
        half.shown = half.full;
        half.center = (16.0, 18.0);
        c.set_effects(&[pass(0, K::Flash, 1.0, 0.0)]);
        c.compose_bgra(SIZE, &[layer(&blue, 2, 2, half, 1.0)], &mut out)
            .unwrap();
        assert!(
            near(px(&out, 8, 18), [255, 0, 0], 2),
            "{:?}",
            px(&out, 8, 18)
        );
        assert!(near(px(&out, 56, 18), [255, 255, 255], 2));
        // 没有图层也照样做（黑底上闪白）
        c.compose_bgra(SIZE, &[], &mut out).unwrap();
        assert!(near(px(&out, 56, 18), [255, 255, 255], 2));
        c.set_effects(&[]);
        c.compose_bgra(SIZE, &[], &mut out).unwrap();
        assert_eq!(px(&out, 56, 18), [0, 0, 0, 255]);
    }

    #[test]
    fn plugin_effects_run_their_wgsl_and_unknown_ones_are_skipped() {
        crate::plugin::register(
            "合成器测试反色",
            "fn plugin_effect(c: vec3<f32>, p: vec2<f32>, dims: vec2<f32>, amount: f32, t: f32) -> vec3<f32> { return mix(c, vec3(1.0) - c, amount); }",
        )
        .unwrap();
        let mut c = compositor();
        let blue = solid(2, 2, [255, 0, 0, 255]);
        let mut out = Vec::new();
        let mut e = pass(1, ve_core::EffectKind::Plugin, 1.0, 0.0);
        e.effect.plugin = ve_core::effect::PluginName::new("合成器测试反色").unwrap();
        c.set_effects(&[e]);
        c.compose_bgra(SIZE, &[layer(&blue, 2, 2, FULL, 1.0)], &mut out)
            .unwrap();
        assert!(
            near(px(&out, 10, 10), [0, 255, 255], 2),
            "蓝反成黄 {:?}",
            px(&out, 10, 10)
        );
        // 没登记的插件：跳过，画面不变
        e.effect.plugin = ve_core::effect::PluginName::new("没有这个插件").unwrap();
        c.set_effects(&[e]);
        c.compose_bgra(SIZE, &[layer(&blue, 2, 2, FULL, 1.0)], &mut out)
            .unwrap();
        assert!(near(px(&out, 10, 10), [255, 0, 0], 2));
    }

    #[test]
    fn spatial_effects_mix_neighbouring_pixels() {
        use ve_core::EffectKind as K;
        let mut c = compositor();
        let tex = red_blue();
        let mut out = Vec::new();
        let run = |comp: &mut Compositor, e: ve_core::EffectPass, buf: &mut Vec<u8>| {
            comp.set_effects(&[e]);
            comp.compose_bgra(SIZE, &[layer(&tex, 64, 36, FULL, 1.0)], buf)
                .unwrap();
        };
        // 模糊：分界处红蓝混在一起，远处不变
        run(&mut c, pass(1, K::Blur, 1.0, 0.0), &mut out);
        let edge = px(&out, 32, 18);
        assert!(edge[0] > 40 && edge[2] > 40, "分界处 {edge:?}");
        assert!(near(px(&out, 2, 18), [0, 0, 255], 3));
        // RGB 色差：分界左边一点点出现蓝（蓝通道往左挪）
        run(&mut c, pass(1, K::ChromaShift, 1.0, 0.0), &mut out);
        let left = px(&out, 31, 18);
        assert!(left[0] > 200 && left[2] > 200, "分界左边 {left:?}");
        // 锐化：纯色区域不变
        run(&mut c, pass(1, K::Sharpen, 1.0, 0.0), &mut out);
        assert!(near(px(&out, 10, 18), [0, 0, 255], 2));
        // 颗粒：纯色上出现起伏，换个时刻噪点不同
        run(&mut c, pass(1, K::Grain, 1.0, 0.0), &mut out);
        let a: Vec<u8> = (0..20).map(|x| px(&out, x, 5)[2]).collect();
        assert!(a.iter().any(|&v| v < 240), "有暗下去的点 {a:?}");
        run(&mut c, pass(1, K::Grain, 1.0, 0.5), &mut out);
        let b: Vec<u8> = (0..20).map(|x| px(&out, x, 5)[2]).collect();
        assert_ne!(a, b);
        // 马赛克：同一个方块里颜色一样
        run(&mut c, pass(1, K::Mosaic, 1.0, 0.0), &mut out);
        assert_eq!(px(&out, 0, 0), px(&out, 1, 1));
        // 强度 0 的马赛克：和不加一样
        run(&mut c, pass(1, K::Mosaic, 0.0, 0.0), &mut out);
        let mosaic_off = out.clone();
        c.set_effects(&[]);
        c.compose_bgra(SIZE, &[layer(&tex, 64, 36, FULL, 1.0)], &mut out)
            .unwrap();
        assert_eq!(mosaic_off, out);
        // 发光：纯红纯蓝都不到发光阈值以上的白，颜色只会更亮不会变暗
        run(&mut c, pass(1, K::Glow, 1.0, 0.0), &mut out);
        assert!(px(&out, 10, 18)[2] >= 250);
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
    fn mirrored_layer_swaps_left_and_right() {
        let mut c = compositor();
        // 4×2：左半红、右半蓝；镜像后左边是蓝、右边是红
        let mut tex = Vec::new();
        for _ in 0..2 {
            tex.extend(solid(2, 1, [0, 0, 255, 255]));
            tex.extend(solid(2, 1, [255, 0, 0, 255]));
        }
        let mirrored = ve_core::Transform {
            mirror: true,
            ..ve_core::Transform::default()
        };
        let g = ve_core::layer_geometry(SIZE, (4, 2), &mirrored);
        let mut out = Vec::new();
        c.compose_bgra(SIZE, &[layer(&tex, 4, 2, g, 1.0)], &mut out)
            .unwrap();
        let (left, right) = (px(&out, 4, 18), px(&out, 60, 18));
        assert!(near(left, [255, 0, 0], 2), "左边应是蓝：{left:?}");
        assert!(near(right, [0, 0, 255], 2), "右边应是红：{right:?}");
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
        let g = geom(0.0, 0.0, 4.0, 2.0);
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
