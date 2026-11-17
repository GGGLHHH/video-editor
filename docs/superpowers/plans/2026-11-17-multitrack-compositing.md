# 多轨合成 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 多条视频轨叠加；片段可调位置 / 缩放 / 裁剪 / 不透明度；预览和导出共用一个 wgpu 显卡合成器。

**Architecture:**
- 新增 `ve-render` crate，负责 wgpu 合成。输入一组从下往上排的 BGRA 预乘图层，输出两种：预览用 BGRA，导出用 BT.709 NV12。
- 几何计算 `layer_geometry` 是纯函数，放在 ve-core，预览、导出、拖拽框三处共用。
- 预览：视频工作线程并行解码各层，交给合成器，产出的 `VideoFrame` 和以前一样，后面的链路不变。
- 导出：自己逐帧渲染出 NV12，经管道交给 ffmpeg 编码；音频和字幕的处理沿用现有滤镜图。

**Tech Stack:** Rust 1.99.1、wgpu 30.0.1（DX12；测试用 WARP）、bytemuck、ffmpeg-next 9.1、gpui-kit 0.10。

**Spec:** `docs/superpowers/specs/2026-11-17-multitrack-compositing-design.md`

## Global Constraints

- wgpu 版本 `30.0.1`，和 Cargo.lock 里 gpui-kit 用的同一个版本；后端默认 DX12；测试中 `RenderConfig { software: true }` 强制用 WARP。
- 测试画布用小尺寸（≤ 128×72），否则 WARP 太慢。
- `Transform` 默认值：`x = 0, y = 0, scale = 1, crop = 0, opacity = 1`。默认值下的画面必须和 MVP 完全一样：等比适配、居中。
- 不变式：所有字段都是有限数；`0.01 ≤ scale ≤ 10`；`0 ≤ opacity ≤ 1`；四边裁剪都 ≥ 0，且 `left + right < 1`、`top + bottom < 1`。
- 裁剪不改变画面的位置和大小：被裁的边直接消失。
- 工程文件版本号 `2`，同时能读第 1 版，缺少的 `transform` 取默认值。
- 轨道顺序就是界面上从上到下的顺序；合成时从最下面一条视频轨开始往上叠。
- 新视频轨放在所有视频轨的最上面；新音频轨放在最后一条音频轨的下面；某种类型的最后一条轨不能删。
- 吸附阈值 8 个屏幕像素。
- 颜色：解码按素材标注的色彩矩阵和范围；没有标注时，高度 ≥ 720 按 BT.709，否则按 BT.601。导出输出 BT.709 有限范围，并标注 `bt709`。
- 性能（1080p60 素材，显卡有负载时也要达标）：
  - 单层播放 ≥ 59.5 帧/秒；
  - 三层播放 ≥ 59.5 帧/秒，显示间隔 > 25 ms 的次数 ≤ 0.5%；
  - 两层拖动预览 p99 ≤ 80 ms；
  - 单层导出不慢于 MVP 基线；
  - 三层导出 ≥ 1 倍实时。
- 覆盖率：ve-render ≥ 90%，其余门槛不变。`scripts/check.ps1` 必须全部通过。
- 按用户规则，不擅自 commit。计划里的「Commit」步骤一律改为「跑 check，并在 ledger 里记一笔」。

## Review Focus

1. 同一素材在两条视频轨上同时出现、时间点不同：两层都要显示各自正确的帧，不能互相抢解码器。
2. 图片层（`still`）叠在视频上，并且延续超出视频的时长：图片之外的部分是黑底，图片不能消失。
3. 最上层的片段先结束，下层还在继续：之后的帧只剩下层，不能残留上层的最后一帧。
4. 导出尺寸和工程尺寸不同（720p 导出 1080p 工程）：位置和比例要和预览一致（换算成比例后误差 ≤ 1 px）。
5. 第 1 版工程文件里只有一条视频轨：打开后画面和以前完全一样，保存后变成第 2 版。

这五项分别在 Task 6、6、6、7、1 的测试中覆盖。

---

### Task 0：测量 MVP 基线

**Files:**
- Create: `crates/ve-export/examples/export_bench.rs`
- Modify: `docs/plan.md`，加一节「多轨合成 · 基线」

**Interfaces:** `export_bench <素材> [层数] [秒数] [--encoder nvenc|x264]` 的工作方式：
1. 用这个素材铺 N 层：第 0 层铺满；第 1 层缩放 0.33、放在右下；第 2 层缩放 0.15、放在左上。
2. 按工程尺寸导出到临时目录。
3. 打印帧数、耗时、帧/秒、几倍实时。

在 Task 7 之前，层数只能是 1，因为 MVP 只支持单层。

- [ ] 写好 `export_bench` 的单层版本，对 `media/test_1080p60.mp4` 跑 30 秒 NVENC 导出，记下帧/秒。
- [ ] 跑 `cargo run --release -p ve-engine --example scrub_latency -- media/test_1080p60.mp4 60`，记下 p99。
- [ ] 两组数字连同当时的显卡占用率写进 `docs/plan.md`。

### Task 1：Transform、几何计算、工程文件第 2 版

**Files:**
- Create: `crates/ve-core/src/transform.rs`
- Modify: `crates/ve-core/src/timeline.rs`（`Clip.transform`、不变式、`InvariantError::BadTransform`）、`crates/ve-core/src/edit.rs`（`set_transform`、构造 Clip 的地方）、`crates/ve-core/src/testing.rs`、`crates/ve-core/src/project_file.rs`（VERSION = 2，接受 1）、`crates/ve-core/src/lib.rs`

**Interfaces（Produces）：**
```rust
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)] #[serde(default)]
pub struct Crop { pub left: f32, pub top: f32, pub right: f32, pub bottom: f32 }   // Default 全 0
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)] #[serde(default)]
pub struct Transform { pub x: f32, pub y: f32, pub scale: f32, pub crop: Crop, pub opacity: f32 }
impl Default for Transform { /* 0,0,1,Crop::default(),1 */ }
impl Transform { pub fn is_valid(&self) -> bool; pub const SCALE_RANGE: (f32, f32) = (0.01, 10.0); }
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect { pub x: f32, pub y: f32, pub w: f32, pub h: f32 }
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerGeometry { pub full: Rect, pub shown: Rect, pub uv: Rect }  // uv 用 0–1 纹理坐标
pub fn layer_geometry(canvas: (u32, u32), source: (u32, u32), t: &Transform) -> LayerGeometry;
// Clip 新增 `#[serde(default)] pub transform: Transform`
pub fn set_transform(project: &mut Project, clip: ClipId, t: Transform) -> Result<(), EditError>;
// 不是视频轨上的媒体片段 → EditError::WrongTrackKind；t 无效 → EditError::InvalidTransform（新增）
```

`layer_geometry` 的计算：先把完整源画面等比放进画布并乘以 scale，得到 `full`，中心在 `(cw/2 + x·cw, ch/2 + y·ch)`；然后 `shown` 从 `full` 四边按 crop 比例往里缩，`uv` 同样按比例缩。内部用 f64 计算。

- [ ] 先写测试（`transform.rs`）：
  - `default_fits_and_centers`：画布 1920×1080、源 1280×720 → `full = shown = (0, 0, 1920, 1080)`，`uv = (0, 0, 1, 1)`；源 1080×1080 → `full = (420, 0, 1080, 1080)`。
  - `scale_and_offset`：画布 1000×500、源 1000×500、scale 0.5、x 0.25、y -0.1 → `full = (500, 75, 500, 250)`。
  - `crop_keeps_unclipped_edges`：crop.left 0.25 → `shown.x = full.x + 0.25·full.w`，`shown` 的右边、上边、下边和 `full` 一致；`uv.x = 0.25`。
  - proptest `geometry_invariants`：只要 Transform 有效，`shown` 就在 `full` 里面；未裁剪时 `full` 和源画面等比，相对误差 < 1e-4。
  - `invalid_transforms_rejected`：NaN、scale 0、opacity 1.5、crop 0.6 + 0.5 都被判为无效。
- [ ] 跑测试，确认失败，然后实现，再跑到通过。
- [ ] 测试 `set_transform_validates_and_targets_video_only`：作用在音频轨片段上 → `WrongTrackKind`；传入无效 t → `InvalidTransform`；有效时写入；`check_invariants` 能拦住无效的 transform。
- [ ] 测试 `loads_v1_file_with_default_transform`：手写一份第 1 版 JSON（片段没有 transform 字段），加载后片段的 transform 等于默认值；保存后 `version == 2`。原来的 `load_rejects_unknown_version` 仍然拒绝 99。
- [ ] `cargo test -p ve-core` 全部通过。

### Task 2：轨道增删

**Files:** Modify `crates/ve-core/src/edit.rs`、`crates/ve-core/src/timeline.rs`

**Interfaces（Produces）：**
```rust
pub fn add_track(project: &mut Project, kind: TrackKind) -> Result<TrackId, EditError>;   // Subtitle → EditError::WrongTrackKind
pub fn remove_track(project: &mut Project, track: TrackId) -> Result<(), EditError>;     // 最后一条同类 → EditError::LastTrack（新增）；不存在 → 现有的 NotFound 类错误
impl Timeline { pub fn video_tracks_bottom_up(&self) -> impl Iterator<Item = &Track>; }
```

- [ ] 测试：
  - `add_video_track_goes_on_top`：MVP 时间轴 [V, A, S] 加一条视频轨 → [V新, V, A, S]；
  - `add_audio_track_goes_below_audio`：加一条音频轨 → [V, A, A新, S]；
  - `remove_track_drops_clips_and_refuses_last`；
  - `bottom_up_order`。
- [ ] 实现，`cargo test -p ve-core` 通过。

### Task 3：解码颜色和透明

**Files:** Modify `crates/ve-media/src/video.rs`、`crates/ve-media/src/testing.rs`（新素材）

**Interfaces：** 对外 API 不变。`VideoFrame.data` 的语义改为 **BGRA 预乘**：不透明素材不受影响；带透明通道的素材（像素格式有 alpha）在缩放后预乘。

- [ ] 新增测试素材：
  - `Bt709Red`：`color=c=0xC02020:s=1280x720`，编成 yuv420p，标注 `-colorspace bt709 -color_range tv`；
  - `Bt601Red`：同样颜色，640×360，不加标注；
  - `PngHalfAlpha`：64×64 PNG，左半边 RGBA(255, 0, 0, 128)，右半边全透明。
- [ ] 测试：
  - `bt709_tagged_source_decodes_to_original_rgb`：中心像素 BGR ≈ (0x20, 0x20, 0xC0)，每个通道 ±3；
  - `untagged_sd_uses_bt601`：同样 ±3；
  - `alpha_png_is_premultiplied`：左半边像素 ≈ (0, 0, 128, 128)，右半边是 (0, 0, 0, 0)。
- [ ] 实现：
  - 建 swscale 上下文后，用 `sws_setColorspaceDetails` 设置源的矩阵和范围（按上面的规则取），目标设为全范围 RGB；
  - 源像素格式有 alpha 时，逐像素预乘；
  - 都要在安全封装里完成。如果 ffmpeg-next 没有提供这个接口，就在 ledger 里写一条 Ruling：改用 `scale` 滤镜图的 `in_color_matrix`。
- [ ] `cargo test -p ve-media` 通过。

### Task 4：ve-render，合成器输出 BGRA

**Files:**
- Create: `crates/ve-render/{Cargo.toml, src/lib.rs, src/compositor.rs, src/shader.wgsl, src/error.rs}`
- Modify: 根 `Cargo.toml`（workspace 依赖 `ve-render`、`wgpu = "30.0.1"`、`bytemuck`、`pollster`）、`scripts/check.ps1`（`'ve-render' = 90`）、`deny.toml`（如果有新的许可证）

**Interfaces（Produces）：**
```rust
pub struct RenderConfig { pub software: bool }
pub enum FramePixels<'a> { Bgra { width: u32, height: u32, data: &'a [u8] } }   // 预乘
pub struct Layer<'a> { pub pixels: FramePixels<'a>, pub geometry: ve_core::LayerGeometry, pub opacity: f32 }
pub struct Compositor { .. }
impl Compositor {
    pub fn new(config: &RenderConfig) -> Result<Self, RenderError>;          // 没有硬件显卡就退回 WARP 并打 log
    pub fn adapter_name(&self) -> &str;
    pub fn compose_bgra(&mut self, size: (u32, u32), layers: &[Layer<'_>], out: &mut Vec<u8>) -> Result<(), RenderError>;
}
#[derive(Debug, thiserror::Error)] pub enum RenderError { NoAdapter, Device(String), BadLayer(String) }
```

只保留 spike 里的做法（`spikes/gpu-compositor/src/compositor.rs`）。混合方式改为预乘：`BlendState::PREMULTIPLIED_ALPHA_BLENDING`，片元输出 `c * opacity`。纹理槽位按层下标复用，尺寸变了就重新分配；输出纹理按 size 重建。`data.len()` 和宽高对不上时返回 `BadLayer`。

- [ ] 测试（全部 `software: true`）：
  - `empty_is_black`；
  - `opaque_layer_lands_on_exact_pixels`：64×36 画布，红色图层 rect (16, 8, 32, 18)，检查内外各一个像素；
  - `opacity_blends`：白色底层 + 黑色上层，不透明度 0.5 → 127 或 128；
  - `crop_uv_samples_subregion`：左红右蓝的图层，uv 只取右半边 → 结果全是蓝色；
  - `premultiplied_edges_not_dark`：半透明红色图层叠在白底上 → 结果 ≈ (255, 127, 127)；
  - `layer_resize_reallocates`；
  - `size_mismatch_is_error`。
- [ ] 实现到全部通过。跑 `cargo deny check` 并通过。

### Task 5：ve-render，输出 NV12

**Files:** Modify `crates/ve-render/src/compositor.rs`，Create `crates/ve-render/src/nv12.wgsl`

**Interfaces（Produces）：**
```rust
pub struct Nv12 { pub width: u32, pub height: u32, pub data: Vec<u8> }   // Y 平面后接 UV 交错平面，紧密排列，宽高为偶数
impl Compositor { pub fn compose_nv12(&mut self, size: (u32, u32), layers: &[Layer<'_>], out: &mut Nv12) -> Result<(), RenderError>; }
```

先合成到 RGBA 中间纹理，再用两遍渲染分别得到 Y（R8，全尺寸）和 UV（Rg8，半尺寸，每 2×2 取平均），使用 BT.709 有限范围系数。宽或高是奇数时返回 `BadLayer`。

- [ ] 测试：
  - `nv12_matches_bt709_formula`：纯色 (200, 100, 50) → Y、U、V 和公式算出的值相差都在 ±1 以内；
  - `nv12_black_and_white`：黑色得到 (16, 128, 128)，白色得到 (235, 128, 128)；
  - `odd_size_rejected`。
- [ ] 实现到通过。`cargo llvm-cov -p ve-render` 达到 90% 或以上。

### Task 6：预览多层合成

**Files:**
- Modify: `crates/ve-engine/src/resolve.rs`、`crates/ve-engine/src/video_worker.rs`、`crates/ve-engine/Cargo.toml`（加 ve-render）、`crates/ve-engine/src/engine.rs`（`EngineConfig.render: RenderConfig`）、`crates/ve-engine/examples/scrub_latency.rs`（加 `--layers N`）
- Modify: `crates/ve-app/src/app.rs`，让 `--bench-playback` 支持 `--layers N`，铺层的方式和 Task 0 一样

**Interfaces（Produces）：**
```rust
pub struct LayerSource { pub track: TrackId, pub asset: AssetId, pub clip: ClipId, pub source_time: Time, pub transform: Transform }
pub fn resolve_layers(project: &Project, t: Time) -> Vec<LayerSource>;   // 从下往上
pub fn preview_canvas(settings: &ProjectSettings, max: ScaleTarget) -> (u32, u32);
```

`resolve_video` 改成返回 `resolve_layers` 的最上层，保留现有的测试。工作线程的变化：
- `decoders` 改用键 `(TrackId, AssetId)`，`failed` 也一样；
- `render(t)` 先解析各层，然后用 `std::thread::scope` 并行解码，每层先查缓存；
- 再用 `layer_geometry`（画布用 `preview_canvas`）和合成器合成，得到 `VideoFrame`；没有任何层时得到黑帧；
- 合成器在工作线程里懒创建；创建失败时退回 MVP 的单层直出，并打 error 日志。

- [ ] 测试（`EngineConfig.render.software = true`，测试素材用 ve-media 里已有的彩条和纯色素材）：
  - `two_layers_compose_pip`：下层纯红铺满，上层纯蓝 scale 0.5 居中 → 帧的中心像素是蓝色，角上是红色；
  - `same_asset_two_tracks_independent`：同一素材在两条轨上，时间差 2 秒，两层取到的帧号不同（验证工作线程内部的解析和解码结果）；
  - `still_over_video_outlives_it`：图片层比视频长，视频结束后的那一帧只有图片，其余部分是黑色；
  - `top_layer_ends_bottom_continues`：上层结束后，帧里只剩下层；
  - `resolve_layers_bottom_up`。
- [ ] 现有引擎测试全部通过（单层画面不变：默认 transform 下，合成结果和原帧逐像素相比差 ≤ 2）。
- [ ] 实测：
  - `scrub_latency --layers 2` 的 p99；
  - `ve-app --bench-playback --layers 1/3/4` 的帧率；
  - 结果写进 `docs/plan.md`，对照性能指标。

### Task 7：导出改由合成器渲染

**Files:**
- Create: `crates/ve-export/src/render.rs`（逐帧渲染管线）
- Modify: `crates/ve-export/src/graph.rs`（视频输入改为管道、字幕作用在 `[0:v]`、颜色标注）、`crates/ve-export/src/run.rs`（进度、取消、回退）、`crates/ve-export/Cargo.toml`（ve-media、ve-render）、`crates/ve-export/examples/export_bench.rs`（多层）
- Test: `crates/ve-export/tests/consistency.rs`

**Interfaces：**
- `export(...)` 的签名不变。
- 新增 `pub fn frame_count(project: &Project) -> u64`。
- `build_job` 不再接视频素材输入：
  - 第 0 个输入是 `-f rawvideo -pix_fmt nv12 -s WxH -r R -i pipe:0`，素材的音频输入从第 1 个开始编号；
  - 有字幕时视频链为 `[0:v]subtitles=subs.ass[vout]`，没有字幕时为 `[0:v]null[vout]`；
  - 输出加上 `-colorspace bt709 -color_primaries bt709 -color_trc bt709`。
- 渲染管线：
  - 每个用到的 (轨道, 片段) 一个顺序解码器，`ScaleTarget` 按设计稿里「导出解码尺寸」的规则取；
  - 合成线程调用 `compose_nv12`；
  - 写入 stdin 的线程和合成线程之间用 `sync_channel(4)`；
  - 每写完一帧报一次进度 `k / total`；
  - 取消时关闭 stdin、杀掉子进程、删除 `.ve-part`。

- [ ] 改写 `graph.rs` 里断言旧视频链的测试，改为断言新的输入和视频链。
- [ ] 一致性测试 `export_matches_preview_three_layers`（`software: true`，640×360，x264 `-qp 0`，2 秒）：
  1. 同一个三层工程，在 1.0 秒处用合成器出一张 BGRA；
  2. 从导出文件里解出第 30 帧；
  3. PSNR ≥ 35 dB，画中画边界的位置误差 ≤ 1 px。
- [ ] 测试 `export_size_differs_from_project`：1280×720 工程导出 640×360，画中画框按比例换算后误差 ≤ 1 px。
- [ ] 现有导出测试（取消、`.ve-part`、MissingMedia、OutputIsInput、NVENC 回退）全部通过。
- [ ] 实测 `export_bench`：
  - 1 层和 3 层各一组，写进 `docs/plan.md`；
  - **单层比基线慢的话，执行 Task 7b。**

### Task 7b（有条件）：解码后直接上传 YUV

只有在单层导出或播放没有达到基线时才做。具体做法：
- ve-media 加 `VideoDecoder::next_frame_yuv`；
- `FramePixels::Yuv420 { y, u, v, matrix, range }`，在着色器里转成 RGB；
- 帧缓存同时支持两种帧格式。

做完重新测量。

### Task 8：界面，轨道头和加删轨

**Files:** Modify `crates/ve-app/src/timeline/{view.rs, geometry.rs, interaction.rs}`、`crates/ve-app/src/workspace.rs`，Test `crates/ve-app/tests/ui.rs`

**Interfaces：**
- 轨道头宽度为常量 `TRACK_HEADER_W`；
- 轨道名：从下往上数，视频轨 V1、V2…，音频轨 A1、A2…，字幕轨 S1；
- 按钮 id：`add-video-track`、`add-audio-track`、`track-del-{i}`；
- 删除带片段的轨道时弹出确认框，按钮 id 为 `confirm-ok` / `confirm-cancel`，沿用现有确认框的写法。

- [ ] UI 测试：
  - `add_and_remove_tracks`：加一条视频轨后视频轨变成 2 条，撤销后恢复 1 条；
  - `deleting_track_with_clips_asks_first`：点「取消」什么都不变，点「确定」轨道被删除；
  - `clip_drags_between_video_tracks`。
- [ ] 现有时间轴测试全部通过：命中测试的坐标要加上轨道头宽度。

### Task 9：属性面板的「画面」区

**Files:** Modify `crates/ve-app/src/inspector.rs`，Test `crates/ve-app/tests/ui.rs`

**Interfaces：**
- 输入框 id：`tf-x`、`tf-y`、`tf-scale`、`tf-crop-l/t/r/b`，单位都是百分比；
- 滑块 id：`tf-opacity`；
- 按钮 id：`tf-reset`；
- 只在选中视频轨上的媒体片段时显示。

- [ ] UI 测试：
  - `transform_fields_edit_clip`：X 输入 25 → `transform.x == 0.25`；
  - 越界的输入被拒绝或夹到范围内，取哪种方式写进 ledger 的 Ruling；
  - `reset_restores_default`；
  - 拖动不透明度滑块算一步撤销。

### Task 10：预览上直接拖动

**Files:**
- Create: `crates/ve-app/src/preview_handles.rs`（纯函数：命中测试、移动、缩放、吸附）
- Modify: `crates/ve-app/src/preview.rs`
- Test: 单元测试和 `ui.rs`

**Interfaces（Produces）：**
```rust
pub enum HandleHit { Body, Corner(Corner), None }
pub fn hit(shown: Rect, p: (f32, f32), corner_px: f32) -> HandleHit;
pub fn drag_move(start: &Transform, canvas: (f32, f32), delta: (f32, f32), snap_px: f32, geom: &LayerGeometry) -> Transform;  // 吸附中心线 / 边缘
pub fn drag_scale(start: &Transform, geom: &LayerGeometry, corner: Corner, p: (f32, f32)) -> Transform;   // 以中心等比，夹在 SCALE_RANGE
```

这里的坐标都是预览屏幕坐标。由调用方把画布坐标换算到屏幕坐标，比例就是 `fit_rect` 的缩放。

- [ ] 单元测试：
  - `move_snaps_to_center`：偏离中心 5 px 时吸附到 x = 0；偏离 9 px 时不吸附；
  - `corner_drag_scales_uniformly`；
  - `scale_clamped`。
- [ ] UI 测试 `drag_in_preview_moves_selected_clip`：选中片段后在预览框里拖动 → `transform.x` 改变；撤销一次后恢复。
- [ ] 预览上画出选中层的边框和四个角的小方块，样式用主题色。

### Task 11：收尾

- [ ] README 的「能做什么」和「性能」一节、`docs/features.md` 的勾选状态、`docs/decisions.md`（决策 22 起，取自设计稿的决策记录和执行中的 Ruling）、`docs/plan.md` 的进度和实测，都更新。
- [ ] `pwsh scripts/check.ps1` 全部通过，覆盖率达到门槛。
- [ ] 实机截图核对：三层工程的预览，以及导出文件抽帧，两边要一致。
- [ ] 做一次全分支审查，修复 Critical 和 Important 级别的问题。
