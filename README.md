# video-editor

用 Rust + GPUI（gpui-kit 0.10）写的桌面视频剪辑器。

> 状态：**MVP 完成**，**多轨合成完成**（2026-11-17）。导入、剪辑、多轨叠加（画中画 / 台标）、字幕、背景音乐、预览、导出、工程存取都能用。

## 能做什么

- 导入视频 / 音频 / 图片：用「导入素材」按钮，或者直接把文件拖进窗口。素材库显示缩略图、时长和分辨率。
- 时间轴：视频轨（带原声）、音频轨（背景音乐）、字幕轨。
  - 视频轨和音频轨可以增删：轨道头左上角「+V」「+A」，每条轨右侧「×」。时间轴上越靠上的视频轨，画面越靠上。
  - 拖动片段可以移动，也可以换到另一条轨；拖片段的边缘可以修剪；位置会吸附到播放头、片段边缘和帧。
  - 支持分割、删除、波纹删除、撤销 / 重做。一次拖拽只算一步撤销。
- 预览：播放、拖动标尺定位、逐帧前进后退，音画同步，字幕直接叠在画面上（白字黑描边）。
- 属性面板：调整片段音量（-60 到 +12 dB）、设置淡入淡出；修改字幕文字。
- 画面属性（视频轨片段）：位置、缩放、裁剪、不透明度，在属性面板「画面」区填数字，或者直接在预览上拖（拖框内移动、拖四个角缩放，靠近中线和边缘会吸附）。在预览上点一下就能选中那一层。
- 带透明的图片（PNG）叠加时保持透明，可以做台标、水印。
- 字幕：在播放头处加字幕，导入 / 导出 SRT。
- 导出 H.264 mp4：可选分辨率（同工程 / 1080p / 720p）、码率、编码器（NVENC 显卡编码或 x264 CPU 编码；NVENC 失败时自动改用 x264）。导出在后台进行，工具栏显示进度，可以取消。
- 工程文件 `.vep`：同时记录素材的绝对路径和相对路径，整个文件夹搬走后仍能找到素材；找不到的素材会列出名字，相关片段显示黑场 / 静音。

## 快捷键

| 键 | 作用 | 键 | 作用 |
|---|---|---|---|
| 空格 | 播放 / 暂停 | S | 在播放头处分割（有选中则只分割选中的片段） |
| L / K | 播放 / 暂停 | Delete | 删除选中 |
| J | 后退 1 秒 | Shift+Delete | 波纹删除 |
| ← / → | 逐帧 | T | 在播放头处加字幕 |
| Home / End | 到开头 / 结尾 | Ctrl+Z / Ctrl+Y | 撤销 / 重做 |
| Ctrl+= / Ctrl+- | 时间轴放大 / 缩小 | Ctrl+滚轮 | 以鼠标位置为中心缩放 |
| 滚轮 | 时间轴左右滚 | 轨道头上滚轮 / Shift+滚轮 | 轨道上下滚 |
| Ctrl+I | 导入素材 | Ctrl+E | 导出 |
| Ctrl+S / Ctrl+Shift+S | 保存 / 另存为 | Ctrl+O / Ctrl+N | 打开 / 新建 |
| Esc | 拖到一半时取消这次拖动（回到原位、不记撤销）；框选到一半时回到框之前；否则取消选择 | Ctrl+A / Ctrl+Shift+A | 全选 / 取消全选 |
| Ctrl+点击片段 | 加选 / 减选 | Shift+点击片段 | 选中上次点的片段到这里之间的一块（跨轨道） |
| 空白处拖动 | 框选（按住 Ctrl / Shift 时在原有选择上加） | 按住 Alt 拖动 | 临时不吸附（时间轴、预览都是） |

选中多个片段时，拖其中任意一个，整组在各自轨道上一起平移。

光标在输入框里时，单键快捷键不生效，打字不会被抢。

## 构建与运行

需要：
- Rust 1.99（仓库里的 `rust-toolchain.toml` 已固定版本）。
- FFmpeg 开发包：本机用的是 scoop 的 `ffmpeg-shared`，`.cargo/config.toml` 里已设置 `FFMPEG_DIR`，换机器时用环境变量覆盖即可。
- `ffmpeg.exe` 和 FFmpeg 的 DLL 要在 PATH 上。
- 构建脚本需要 libclang，并设置好 `LIBCLANG_PATH`。

```sh
cargo run --release -p ve-app                         # 启动
cargo run --release -p ve-app -- 工程.vep              # 打开工程
cargo run --release -p ve-app -- a.mp4 bgm.m4a        # 导入素材并依次放进时间轴
```

## 质量检查

```sh
pwsh scripts/check.ps1            # fmt、clippy（pedantic+nursery+cargo+部分 restriction，-D warnings）、
                                  # 测试、文档、cargo-deny、cargo-machete、覆盖率门槛
pwsh scripts/check.ps1 -NoCoverage
```

覆盖率门槛：
- ve-core / ve-media / ve-engine / ve-export 每个 crate 的行覆盖率不低于 90%。
- 全工作区不低于 80%。

## 性能测量

```sh
# 完整应用里播放 N 秒，帧率写入 ve-bench.log
cargo run --release -p ve-app -- --bench-playback media/test_1080p60.mp4 --seconds 30
# 拖动预览延迟（随机定位 N 次，可选层数）
cargo run --release -p ve-engine --example scrub_latency -- media/test_1080p60.mp4 60 2
# 多层播放：--layers N
cargo run --release -p ve-app -- --bench-playback media/test_1080p60.mp4 --seconds 20 --layers 3
# 导出测速：素材 层数 秒数
cargo run --release -p ve-export --example export_bench -- media/test_1080p60.mp4 3 30
# 端到端演示：拼工程 → 保存 .vep → 导出 mp4
cargo run --release -p ve-export --example demo -- 视频 音频 输出目录
```

2026-11-17 在 RTX 5090 上的实测：

| 项目 | 结果 |
|---|---|
| 1080p60 播放 30 秒 | 59.97 fps |
| 4K60 HEVC 播放 20 秒 | 59.67 fps |
| 拖动预览延迟，关键帧间隔 2 秒 | p99 40 ms |
| 拖动预览延迟，关键帧间隔约 4 秒 | 最长 76 ms |

多轨合成（2026-11-17，游戏同时在运行、显卡占用 84–98%，按最坏情况看）：

| 项目 | 结果 |
|---|---|
| 播放 1 / 3 / 4 层 1080p60 | 58.4 / 57.3 / 52.7 帧/秒（窗口没被遮挡时） |
| 拖动预览 1 / 2 层 | p99 95 / 141 ms |
| 导出 1 层 / 3 层（NVENC） | 277 帧/秒（MVP 275）/ 243 帧/秒（4 倍实时） |

## 代码结构

```
crates/
  ve-core    纯模型：时间（flicks）、工程 / 轨道 / 片段、编辑操作、吸附、撤销重做、SRT、工程文件
  ve-media   FFmpeg 进程内：探测、视频解码（精确定位 + 缩放 BGRA）、音频解码、缩略图、波形
  ve-engine  播放：时间轴解析、混音、帧缓存、音视频工作线程、声卡输出、以声音为准的时钟
  ve-render  显卡合成（wgpu）：多层画面按位置 / 缩放 / 裁剪 / 不透明度叠加，输出 BGRA（预览）或 NV12（导出）
  ve-export  导出：合成器逐帧出画面经管道交给 ffmpeg 编码，ASS 字幕和混音走 ffmpeg 滤镜图
  ve-app     GPUI 界面：主窗口、素材库、预览、时间轴、属性、导出对话框
```

依赖只能往下：app → engine / export → render / media → core。

## 文档

| 文档 | 内容 |
|---|---|
| [docs/features.md](docs/features.md) | 剪辑软件功能全景，标了 MVP 范围 |
| [docs/superpowers/specs/2026-11-17-mvp-design.md](docs/superpowers/specs/2026-11-17-mvp-design.md) | 设计稿：范围、技术路线、逐帧显示实验 |
| [docs/superpowers/plans/2026-11-17-mvp.md](docs/superpowers/plans/2026-11-17-mvp.md) | 实现计划 |
| [docs/superpowers/specs/2026-11-17-multitrack-compositing-design.md](docs/superpowers/specs/2026-11-17-multitrack-compositing-design.md) | 多轨合成设计稿 |
| [docs/superpowers/plans/2026-11-17-multitrack-compositing.md](docs/superpowers/plans/2026-11-17-multitrack-compositing.md) | 多轨合成实现计划 |
| [spikes/gpu-compositor/README.md](spikes/gpu-compositor/README.md) | 显卡合成实验 |
| [docs/decisions.md](docs/decisions.md) | 决策记录 |
| [docs/plan.md](docs/plan.md) | 进度 |

## 许可证

本项目源码以 [Apache-2.0](LICENSE) 许可证发布。

本仓库不包含 FFmpeg。运行时用到的 FFmpeg 库和 `ffmpeg.exe` 按它们自己的许可证（LGPL / GPL）提供。如果要把程序和带 GPL 组件（如 x264）的 FFmpeg 构建打包一起分发，需要同时遵守 GPL：附上 FFmpeg 的许可证，并提供对应源码。
