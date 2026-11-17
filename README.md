# video-editor

用 Rust + GPUI（gpui-kit 0.10）写的桌面视频剪辑器。

> 状态：**MVP 完成**（2026-11-17）。导入、剪辑、字幕、背景音乐、预览、导出、工程存取都能用。

## 能做什么

- 导入视频 / 音频 / 图片：用「导入素材」按钮，或者直接把文件拖进窗口。素材库显示缩略图、时长和分辨率。
- 时间轴有三条轨：视频轨（带原声）、音频轨（背景音乐）、字幕轨。
  - 拖动片段可以移动，也可以换到另一条轨；拖片段的边缘可以修剪；位置会吸附到播放头、片段边缘和帧。
  - 支持分割、删除、波纹删除、撤销 / 重做。一次拖拽只算一步撤销。
- 预览：播放、拖动标尺定位、逐帧前进后退，音画同步，字幕直接叠在画面上（白字黑描边）。
- 属性面板：调整片段音量（-60 到 +12 dB）、设置淡入淡出；修改字幕文字。
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
| Ctrl+I | 导入素材 | Ctrl+E | 导出 |
| Ctrl+S / Ctrl+Shift+S | 保存 / 另存为 | Ctrl+O / Ctrl+N | 打开 / 新建 |

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
# 拖动预览延迟（随机定位 N 次）
cargo run --release -p ve-engine --example scrub_latency -- media/test_1080p60.mp4 60
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

## 代码结构

```
crates/
  ve-core    纯模型：时间（flicks）、工程 / 轨道 / 片段、编辑操作、吸附、撤销重做、SRT、工程文件
  ve-media   FFmpeg 进程内：探测、视频解码（精确定位 + 缩放 BGRA）、音频解码、缩略图、波形
  ve-engine  播放：时间轴解析、混音、帧缓存、音视频工作线程、声卡输出、以声音为准的时钟
  ve-export  导出：ASS 字幕 + ffmpeg 滤镜图，后台运行并解析进度
  ve-app     GPUI 界面：主窗口、素材库、预览、时间轴、属性、导出对话框
```

依赖只能往下：app → engine / export → media → core。

## 文档

| 文档 | 内容 |
|---|---|
| [docs/features.md](docs/features.md) | 剪辑软件功能全景，标了 MVP 范围 |
| [docs/superpowers/specs/2026-11-17-mvp-design.md](docs/superpowers/specs/2026-11-17-mvp-design.md) | 设计稿：范围、技术路线、逐帧显示实验 |
| [docs/superpowers/plans/2026-11-17-mvp.md](docs/superpowers/plans/2026-11-17-mvp.md) | 实现计划 |
| [docs/decisions.md](docs/decisions.md) | 决策记录 |
| [docs/plan.md](docs/plan.md) | 进度 |

## 许可证

本项目源码以 [Apache-2.0](LICENSE) 许可证发布。

本仓库不包含 FFmpeg。运行时用到的 FFmpeg 库和 `ffmpeg.exe` 按它们自己的许可证（LGPL / GPL）提供。如果要把程序和带 GPL 组件（如 x264）的 FFmpeg 构建打包一起分发，需要同时遵守 GPL：附上 FFmpeg 的许可证，并提供对应源码。
