<#
.SYNOPSIS
  下载固定版本的 FFmpeg（gyan.dev 9.0.2 完整共享版，GPLv3），校验 SHA-256 后解压。
  CI 和打包都用它；在本机跑也行（默认放进 target/ffmpeg，不碰 scoop 装的那份）。
.OUTPUTS
  解压后的 FFmpeg 根目录（含 bin/ include/ lib/）。在 GitHub Actions 里还会把它写进
  FFMPEG_DIR，并把 bin/ 加进 PATH（构建、测试、导出都要用）。
.EXAMPLE
  pwsh scripts/setup-ffmpeg.ps1
#>
param(
    [string]$Dest = (Join-Path $PSScriptRoot '..\target\ffmpeg')
)
$ErrorActionPreference = 'Stop'

# 和本机开发用的 scoop ffmpeg-shared 同一个包；换版本时三项一起改
$Version = '9.0.2'
$Url = "https://github.com/GyanD/codexffmpeg/releases/download/$Version/ffmpeg-$Version-full_build-shared.7z"
$Sha256 = '4d2060a8b34a940aa47d785142055bb92a63053781e55f2ace4546edd519a8f5'

New-Item -ItemType Directory -Force $Dest | Out-Null
$Dest = (Resolve-Path $Dest).Path
$archive = Join-Path $Dest "ffmpeg-$Version-full_build-shared.7z"
$root = Join-Path $Dest "ffmpeg-$Version-full_build-shared"

function Test-Archive {
    (Test-Path $archive) -and ((Get-FileHash $archive -Algorithm SHA256).Hash -eq $Sha256)
}

if (-not (Test-Archive)) {
    Write-Host "下载 FFmpeg $Version ..."
    $ProgressPreference = 'SilentlyContinue'
    Invoke-WebRequest $Url -OutFile $archive
    if (-not (Test-Archive)) {
        throw "FFmpeg 压缩包的 SHA-256 不对（应为 $Sha256），可能下载不完整或被替换"
    }
}
if (-not (Test-Path (Join-Path $root 'bin\ffmpeg.exe'))) {
    & 7z x $archive "-o$Dest" -y | Out-Null
    if ($LASTEXITCODE -ne 0) { throw '7z 解压失败' }
}

if ($env:GITHUB_ENV) {
    "FFMPEG_DIR=$root" | Out-File $env:GITHUB_ENV -Append -Encoding utf8
    (Join-Path $root 'bin') | Out-File $env:GITHUB_PATH -Append -Encoding utf8
}
$root
