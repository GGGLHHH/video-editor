# MVP 设计稿

- 日期：2026-11-17
- 状态：**已确认**（2026-11-17 用户：「就照着现有的推荐，模块化开发，拓展性好足够，测试覆盖率要高，配置 rust 生态最严厉的 lint 和代码校验，我们对性能敏感，直到完成 mvp」）
- 实现计划：`docs/superpowers/plans/2026-11-17-mvp.md`

## 为什么做

想要一个自己能掌控的剪辑器，用 Rust + GPUI 写。长远看要能和本机已有的翻译配音流程对接。

> 待补：现成的剪映 / DaVinci 具体哪里不合用 —— 这会影响 MVP 之后先做什么。

## MVP 范围（提议）

**要做**

1. 导入 mp4 / mov 视频和常见音频文件，素材列表带缩略图
2. 时间轴：1 条视频轨（含原声）+ 1 条背景音乐轨 + 1 条字幕轨
3. 编辑：切割、修剪、移动、删除、波纹删除、吸附；全部可撤销 / 重做
4. 预览：播放 / 暂停、拖动预览、逐帧步进，音画同步
5. 音频：片段音量、淡入淡出，两条音轨实时混音
6. 字幕：手打字幕片段（基础样式），导入 / 导出 SRT
7. 导出：H.264 mp4（NVENC），后台渲染带进度
8. 工程文件保存 / 打开；基础快捷键（空格、J/K/L、S 切割、Ctrl+Z/Y）

**不做（MVP 之后）**

多视频轨叠加、转场、特效、调色、关键帧、变速、代理文件、AI 功能。

**失败标准（技术上能跑但算失败）**

- 预览卡顿、拖动时画面跟不上 —— 剪辑器手感的底线
- 音画不同步

## 技术路线

难点只在**预览播放**；时间轴、撤销、导出主要是工作量。

### A. FFmpeg 解码 + cpal 播声音 + GPUI 图片元素显示（推荐）

- 整条流水线自己掌控，多轨合成、特效、配音对接都沿这条路长，不用推倒重来；FFmpeg 已在配音项目里用熟
- 代价：音画同步、拖动帧缓存要自己写
- **风险**：每帧经 GPUI 贴图通道重新上传，Windows 上 1080p 能否稳定流畅未知 → 先做实验（下文）

### B. GStreamer（[gpui-video-player](https://lib.rs/crates/gpui-video-player)；OpenCut 的 GPUI 版同路）

- 播放、同步、seek 现成；GStreamer 还自带剪辑后端 GES（Pitivi 在用），时间轴 / 导出也有
- 代价：Windows 要附带几百 MB 运行时，问题要钻进 C 库查；该库 Windows 上同样走软件贴图，A 的风险它也有

### C. 嵌入 libmpv 播放窗口

- 播放最顺，硬解、同步、ASS 字幕全现成
- 代价：只能播「一条」拼接好的视频；多轨叠加 / 特效要重写预览，视频区域上不能叠 GPUI 控件。MVP 够用但是死路

### 选择

**A**（2026-11-17 用户选定）。B / C 省的是第一个月，代价是后面扩展受限；A 唯一的大风险已用下面的实验验证通过。

## 实验结果：GPUI 逐帧显示（2026-11-17，**通过**）

代码 `spikes/gpui-frames/`（一次性，不进正式代码），日志在其 `logs/`（不入库）。
环境：gpui 0.2.2（crates.io），Windows 10，RTX 5090，窗口 1600×900、画面等比缩放；测前显卡无其他重负载（基线占用 25–33%，来自桌面常驻程序）。
做法：ffmpeg 子进程出 BGRA 原始帧 → 每帧包成新的 `RenderImage` → `img()` 显示；换帧时对「上上帧」调 `window.drop_image()` 从贴图图集删掉。

| 场景 | 时长 | 显示帧率 | 卡顿（间隔 > 25ms） | 内存 | 结论 |
|---|---|---|---|---|---|
| 1080p60，管道直出 | 5 分钟 | 平均 60.5，最低 42（1 秒） | 4 次，其中 73ms / 344ms 两次与测量期间截屏、抽帧同时 | 工作集 90→91MB，私有 206MB 不变；**进程显存 88MB 不变** | 通过 |
| 1080p60，管道直出，无干扰复测 | 2 分钟 | 平均 60.6，最低 60 | 1 次（29.7ms） | 90→90MB | 通过 |
| 4K60，管道直出 | 2 分钟 | 42.5 | —— | 不涨 | **瓶颈在管道**，不在 GPUI（见下） |
| 4K60，预读 60 帧进内存循环送 | 2 分钟 | 平均 60.7，最低 60 | **0 次**，最大间隔 19.5ms | 不涨（2GB 是预读缓存本身） | 通过 |

要点：

1. **GPUI 逐帧换图在 Windows 上可行**，1080p60 和 4K60 都能稳定跑满；用 `drop_image` 及时删旧帧后，内存、显存都不涨。
2. **4K 下 ffmpeg 子进程管道是瓶颈**：ffmpeg 本身 4K→BGRA 能出 185 fps，经 Windows 管道读只有 42.5 fps（每帧 33MB）。正式版要**把 FFmpeg 库链进进程**解码，不走管道。
3. 「后备：fork GPUI 加显卡贴图元素」**目前不需要**。以后若要硬解零拷贝（省 CPU 转换和上传）再考虑。
4. 测量局限：帧时间取自 `render()` 被调用的时刻，不是真正上屏时刻；肉眼观感需要用户看一眼确认。

## 原实验设计（存档）

- **只改一个变量**：FFmpeg 解出 1080p 帧（RGBA/BGRA），每帧交给 GPUI 显示
- **成功信号**：连播 5 分钟，显示帧率稳定 ≥ 片源帧率；进程内存不持续上涨；显卡占用合理
- **失败信号**：掉帧明显、内存随时间涨（贴图图集不回收）
- **测量**：界面里叠一个帧率 / 丢帧计数，日志里记每秒帧数和内存；测前先确认没有游戏占用显卡
- **失败后备**：fork GPUI，在其 DirectX 11 渲染层加一个直接用显卡贴图的元素（可接 D3D11VA / NVDEC 硬解零拷贝），工作量届时评估

## 工程要求（用户定）

- **模块化、可扩展**：按职责拆 crate，模型层不依赖 UI / FFmpeg；轨道是通用列表，MVP 只是固定建三条
- **测试覆盖率高**：非 UI crate 行覆盖率 ≥ 90%，全工作区 ≥ 80%（`scripts/check.ps1` 卡门槛）；UI 用 gpui-kit 的无界面窗口集成测试
- **最严 lint**：rustc 严格 lint + clippy `pedantic` / `nursery` / `cargo` 全开，再挑一批 `restriction`（禁 unwrap/expect/panic/print 等，测试里放开）；`-D warnings`；`cargo fmt --check`、`cargo doc -D warnings`、`cargo deny`、`cargo machete`
- **性能敏感**：播放跑满片源帧率、拖动预览跟手；热路径不加锁等待、不在 UI 线程解码；解码直接缩放到预览尺寸

## 架构（2026-11-17 定）

```
crates/
  ve-core    纯模型：时间（flicks 整数）、工程 / 素材 / 轨道 / 片段、编辑操作、吸附、撤销重做、SRT、工程文件
  ve-media   FFmpeg 进程内：探测、视频解码（精确 seek + 缩放 BGRA）、音频解码（f32 立体声 48k）、缩略图、波形
  ve-engine  播放：时间轴 → 取帧 / 取音频的解析、混音、帧缓存、视频 / 音频工作线程、cpal 输出、音频主时钟
  ve-export  导出：生成 ASS 字幕 + ffmpeg 滤镜图命令，跑 ffmpeg 可执行文件并解析进度
  ve-app     GPUI 界面（gpui-kit 0.10）：素材库、预览、时间轴、属性面板、菜单快捷键
```

依赖方向只能往下：app → engine / export → media → core；core 不依赖任何 IO。

关键选择（理由见 decisions.md）：
- UI 用 **gpui-kit 0.10**（带 gpui 0.3 快照 + 组件库 + 无界面 UI 测试），不用裸 gpui 0.2.2
- 时间用 **flicks**（1/705 600 000 秒）存 i64：常见帧率和采样率都能整除，无浮点误差
- 撤销重做用**整棵时间轴快照**（片段用 Arc 共享，复制便宜），拖拽这类连续操作合并成一步
- 导出调 **ffmpeg 可执行文件**（与链接的库同一套 shared 构建），滤镜图拼接；预览解码才走进程内库

## 留给实现时决定

- FFmpeg 库怎么接进来：`ffmpeg-next` / `rsmpeg` 绑定，链 gyan.dev 的 shared 开发包或 vcpkg 构建；发布时随程序带 dll
- GPUI 依赖方式：跟 zed 仓库固定 commit，还是用 crates.io 发布版（实验用的是 crates.io 0.2.2，已够用）
- 工程文件格式（倾向 JSON / RON，人能读、好 diff）
- 撤销 / 重做的结构（命令模式 vs 状态快照）
- 字幕渲染：预览时用 GPUI 文字叠加，导出时转 ASS 交给 FFmpeg 烧录，还是两边共用一套光栅化

## 参考

- [gpui-video-player](https://lib.rs/crates/gpui-video-player) — GPUI + GStreamer 播放器
- [awesome-gpui](https://upd.dev/zed-industries/awesome-gpui) — 收录了 OpenCut 的 GPUI 版本
- [OpenCut 现状说明](https://mer.vin/2026/07/opencut-explained-open-source-capcut-alternative-classic-editor-vs-ground-up-rewrite/)
- [Gausian native editor](https://gh.hhj520.top/gausian-AI/Gausian_native_editor) — egui + wgpu，时间轴设计可参考
- [GPUI + wgpu 剪辑原型（HN）](https://hnshowcase.com/post/47824570/building-an-agentic-video-editing-prototype-in-rust-using-gp)
