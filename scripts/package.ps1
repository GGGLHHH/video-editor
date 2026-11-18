<#
.SYNOPSIS
  把发布版程序和 FFmpeg 打成一个解压即用的 Windows 压缩包：dist/video-editor-<版本>-windows-x64.zip。
  先 `cargo build --release -p ve-app`，FFmpeg 用 scripts/setup-ffmpeg.ps1 准备的那份。
.EXAMPLE
  pwsh scripts/package.ps1 -FfmpegDir (pwsh scripts/setup-ffmpeg.ps1)
#>
param(
    [string]$FfmpegDir = $env:FFMPEG_DIR,
    [string]$TargetDir = $(if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }),
    [string]$Out = 'dist'
)
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

if (-not $FfmpegDir -or -not (Test-Path (Join-Path $FfmpegDir 'bin\ffmpeg.exe'))) {
    throw "找不到 FFmpeg（$FfmpegDir）：先运行 scripts/setup-ffmpeg.ps1"
}
$exe = Join-Path $TargetDir 'release\ve-app.exe'
if (-not (Test-Path $exe)) { throw "找不到 $exe：先 cargo build --release -p ve-app" }

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
$name = "video-editor-$version-windows-x64"
$stage = Join-Path $Out $name
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force $stage, (Join-Path $stage 'licenses') | Out-Null

# 程序 + FFmpeg 的 DLL（解码）和 ffmpeg.exe（导出）。程序找 ffmpeg.exe 时先看自己所在的目录
Copy-Item $exe (Join-Path $stage 'video-editor.exe')
Copy-Item (Join-Path $FfmpegDir 'bin\*.dll') $stage
Copy-Item (Join-Path $FfmpegDir 'bin\ffmpeg.exe') $stage

Copy-Item LICENSE (Join-Path $stage 'licenses\video-editor-Apache-2.0.txt')
Copy-Item (Join-Path $FfmpegDir 'LICENSE') (Join-Path $stage 'licenses\FFmpeg-GPLv3.txt')
$readme = @"
视频剪辑器 $version（Windows x64）

解压后双击 video-editor.exe 运行，不用安装。导出时优先用 NVIDIA 显卡编码，没有就用 CPU。

授权
- 视频剪辑器的源码按 Apache-2.0 授权：https://github.com/GGGLHHH/video-editor
- 这个压缩包附带了 FFmpeg（gyan.dev 构建的 9.0.2 完整版，GPLv3），程序运行时链接它的 DLL，
  所以整个压缩包按 GPLv3 分发。授权全文见 licenses/。
- FFmpeg 源码：https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz
  这份构建的说明和所用组件：https://github.com/GyanD/codexffmpeg/releases/tag/9.0.2
"@
Set-Content -Path (Join-Path $stage '说明.txt') -Value $readme -Encoding utf8

$zip = Join-Path $Out "$name.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
(Resolve-Path $zip).Path
