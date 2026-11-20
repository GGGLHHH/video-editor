# 视频剪辑器的常用命令（https://just.systems）。
# `just` 列出全部命令，`just <命令> [参数]` 运行。路径里有空格时加引号：just run '"我的 工程.vep"'

set windows-shell := ["pwsh", "-NoLogo", "-NoProfile", "-Command"]

# 列出全部命令
default:
    @just --list --unsorted

# ── 运行 ──────────────────────────────────────────────

# 启动（发布版）；可以带工程或素材：just run 工程.vep / just run a.mp4 bgm.m4a
run *args:
    cargo run --release -p ve-app -- {{ args }}

# 启动（调试版：编译快、运行慢）
dev *args:
    cargo run -p ve-app -- {{ args }}

# ── 检查 ──────────────────────────────────────────────

# 全量检查（提交前跑，和 CI 一样）：格式、clippy、测试、文档、依赖审计、无用依赖、覆盖率门槛
check:
    pwsh scripts/check.ps1

# 全量检查，但不算覆盖率（快一半）
check-fast:
    pwsh scripts/check.ps1 -NoCoverage

# 格式化代码
fmt:
    cargo fmt --all

# clippy（和 CI 一样严）
lint:
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

# 跑测试，可以按名字过滤：just test stabiliz
test *filter:
    cargo test --workspace --all-features --locked {{ filter }}

# 只跑界面集成测试，可以按名字过滤：just ui otio
ui *filter:
    cargo test -p ve-app --test ui {{ filter }}

# 生成并打开 API 文档
doc:
    cargo doc --workspace --no-deps --open

# ── 性能 ──────────────────────────────────────────────

# 播放帧率（结果写进 ve-bench.log）：just bench-play media/test_1080p60.mp4 30 3
bench-play file="media/test_1080p60.mp4" seconds="30" layers="1":
    cargo run --release -p ve-app -- --bench-playback {{ file }} --seconds {{ seconds }} --layers {{ layers }}

# 拖动预览延迟：随机定位 count 次
bench-scrub file="media/test_1080p60.mp4" count="60" layers="1":
    cargo run --release -p ve-engine --example scrub_latency -- {{ file }} {{ count }} {{ layers }}

# 导出测速
bench-export file="media/test_1080p60.mp4" layers="3" seconds="30":
    cargo run --release -p ve-export --example export_bench -- {{ file }} {{ layers }} {{ seconds }}

# ── 打包与发版 ────────────────────────────────────────

# 本机打 Windows 压缩包 → dist/video-editor-<版本>-windows-x64.zip（会重新编译一遍发布版）
package:
    $ff = pwsh scripts/setup-ffmpeg.ps1; $env:RUSTFLAGS = '-C target-feature=+crt-static'; cargo build --release -p ve-app; if ($LASTEXITCODE) { exit $LASTEXITCODE }; pwsh scripts/package.ps1 -FfmpegDir $ff

# 看某个版本的发版说明（取自 CHANGELOG.md）：just notes 0.2.0
notes version:
    pwsh scripts/release-notes.ps1 -Version {{ version }}

# 发版：先把 CHANGELOG.md 的「未发布」写好，再 just release 0.3.0（-DryRun 只预览，-Yes 不问）
release version *flags:
    pwsh scripts/release.ps1 -Version {{ version }} {{ flags }}
