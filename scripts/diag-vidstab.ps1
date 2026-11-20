<#
  临时诊断（PR #2）：CI 上视频防抖偶尔失败——ffmpeg 退出码非零却不输出任何错误。
  在 CI 机器上把防抖用的两遍 ffmpeg（vidstabdetect、vidstabtransform，参数和 ve-export 里一样）
  各跑若干次、4 个并行，统计退出码；再把 OMP_NUM_THREADS 设为 1 跑同样次数，只改这一个变量。
  查清后删掉这个脚本和 CI 里调用它的那一步。
#>
param(
    [int]$Runs = 30,
    [string]$Work = (Join-Path ([IO.Path]::GetTempPath()) "diag-vidstab")
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force $Work | Out-Null
$src = Join-Path $Work 'in.mp4'
& ffmpeg -hide_banner -loglevel error -y -f lavfi -i 'testsrc=size=64x36:rate=30' `
    -f lavfi -i 'sine=frequency=440:sample_rate=48000' -t 2 -c:v libx264 -pix_fmt yuv420p -c:a aac $src
if ($LASTEXITCODE -ne 0) { throw "生成测试视频失败：$LASTEXITCODE" }
ffmpeg -hide_banner -version | Select-Object -First 1
"CPU: $((Get-CimInstance Win32_Processor | Select-Object -First 1).Name)，逻辑核 $([Environment]::ProcessorCount)"

function Invoke-Batch([string]$Label, [string]$Omp) {
    $jobs = foreach ($i in 1..$Runs) {
        Start-ThreadJob -ThrottleLimit 4 -ArgumentList $i, $src, $Work, $Omp -ScriptBlock {
            param($i, $src, $Work, $Omp)
            if ($Omp) { $env:OMP_NUM_THREADS = $Omp } else { Remove-Item Env:OMP_NUM_THREADS -ErrorAction SilentlyContinue }
            $dir = Join-Path $Work "run-$Omp-$i"
            New-Item -ItemType Directory -Force $dir | Out-Null
            Push-Location $dir
            try {
                $detect = & ffmpeg -hide_banner -loglevel error -y -ss 0.5 -t 1 -i $src `
                    -vf 'vidstabdetect=shakiness=6:accuracy=15:result=t.trf' -f null - 2>&1
                $d = $LASTEXITCODE
                $out = Join-Path $dir 'out.mp4'
                $transform = & ffmpeg -hide_banner -loglevel error -y -ss 0.5 -t 1 -i $src `
                    -map 0:v:0 -map '0:a:0?' `
                    -vf 'vidstabtransform=input=t.trf:smoothing=12:optzoom=1:interpol=bicubic,unsharp=5:5:0.6:3:3:0.3' `
                    -c:v libx264 -preset veryfast -crf 16 -pix_fmt yuv420p -c:a aac -b:a 256k $out 2>&1
                $t = $LASTEXITCODE
                [pscustomobject]@{
                    Detect    = '0x{0:X8}' -f $d
                    Transform = '0x{0:X8}' -f $t
                    OutFile   = Test-Path $out
                    Said      = (($detect + $transform) | Out-String).Trim()
                }
            } finally { Pop-Location }
        }
    }
    $results = $jobs | Wait-Job | Receive-Job
    $jobs | Remove-Job
    "== $Label（$Runs 次）=="
    $results | Group-Object Detect | ForEach-Object { "  分析   退出码 $($_.Name)：$($_.Count) 次" }
    $results | Group-Object Transform | ForEach-Object { "  稳定   退出码 $($_.Name)：$($_.Count) 次" }
    $bad = $results | Where-Object { $_.Detect -ne '0x00000000' -or $_.Transform -ne '0x00000000' }
    foreach ($b in $bad | Select-Object -First 5) {
        "  失败：分析 $($b.Detect) 稳定 $($b.Transform) 输出文件在不在 $($b.OutFile) 说了：$($b.Said)"
    }
}

Invoke-Batch 'OpenMP 默认线程数' ''
Invoke-Batch 'OMP_NUM_THREADS=1' '1'
