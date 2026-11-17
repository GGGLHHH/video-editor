<#
.SYNOPSIS
  全量代码校验：格式、clippy（最严）、测试、文档、依赖审计、无用依赖、覆盖率门槛。
.EXAMPLE
  pwsh scripts/check.ps1            # 全部
  pwsh scripts/check.ps1 -NoCoverage
#>
param(
    [switch]$NoCoverage
)
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

# 覆盖率门槛（行覆盖率，%）
$CrateThresholds = @{
    've-core'   = 90
    've-media'  = 90
    've-engine' = 90
    've-export' = 90
}
$WorkspaceThreshold = 80

function Invoke-Step([string]$Name, [scriptblock]$Body) {
    Write-Host "==> $Name" -ForegroundColor Cyan
    & $Body
    if ($LASTEXITCODE -ne 0) {
        Write-Host "FAILED: $Name" -ForegroundColor Red
        exit 1
    }
}

Invoke-Step 'fmt' { cargo fmt --all --check }
Invoke-Step 'clippy' { cargo clippy --workspace --all-targets --all-features --locked -- -D warnings }
Invoke-Step 'test' { cargo test --workspace --all-features --locked }
Invoke-Step 'doc' {
    $env:RUSTDOCFLAGS = '-D warnings'
    cargo doc --workspace --no-deps --document-private-items --locked
    Remove-Item Env:RUSTDOCFLAGS
}
Invoke-Step 'deny' { cargo deny check }
Invoke-Step 'machete' { cargo machete }

if (-not $NoCoverage) {
    $json = 'target/llvm-cov-summary.json'
    Invoke-Step 'coverage' {
        cargo llvm-cov --workspace --all-features --locked --json --summary-only --ignore-filename-regex 'testing\.rs$' --output-path $json
    }
    $data = Get-Content $json -Raw | ConvertFrom-Json
    $failed = $false
    $perCrate = @{}
    foreach ($f in $data.data[0].files) {
        $norm = $f.filename -replace '\\', '/'
        if ($norm -match '/crates/([^/]+)/src/') {
            $c = $Matches[1]
            if (-not $perCrate.ContainsKey($c)) { $perCrate[$c] = @(0, 0) }
            $perCrate[$c][0] += $f.summary.lines.covered
            $perCrate[$c][1] += $f.summary.lines.count
        }
    }
    foreach ($c in ($perCrate.Keys | Sort-Object)) {
        $cov, $cnt = $perCrate[$c]
        $pct = if ($cnt -gt 0) { [math]::Round(100.0 * $cov / $cnt, 2) } else { 100 }
        $need = $CrateThresholds[$c]
        $mark = ''
        if ($null -ne $need -and $pct -lt $need) { $mark = "  < $need  FAIL"; $failed = $true }
        Write-Host ("{0,-10} {1,7}%  ({2}/{3}){4}" -f $c, $pct, $cov, $cnt, $mark)
    }
    $total = $data.data[0].totals.lines.percent
    Write-Host ("workspace  {0,7:N2}%" -f $total)
    if ($total -lt $WorkspaceThreshold) { Write-Host "workspace < $WorkspaceThreshold FAIL" -ForegroundColor Red; $failed = $true }
    if ($failed) { exit 1 }
}

Write-Host 'ALL CHECKS PASSED' -ForegroundColor Green
