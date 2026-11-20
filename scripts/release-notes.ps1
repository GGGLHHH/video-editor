<#
  从 CHANGELOG.md 取某个版本的那一段，当 GitHub Release 的说明。
  用法：pwsh scripts/release-notes.ps1 -Version 0.2.0 [-Out notes.md]
  （-Version 也认 v0.2.0。）CHANGELOG 里没有这个版本时报错：发版前先把「未发布」整理成这个版本。
#>
param(
    [Parameter(Mandatory)] [string]$Version,
    [string]$Changelog = (Join-Path $PSScriptRoot '..\CHANGELOG.md'),
    [string]$Out
)
$ErrorActionPreference = 'Stop'
$v = $Version.TrimStart('v')
$lines = Get-Content -LiteralPath $Changelog -Encoding utf8
$start = -1
for ($i = 0; $i -lt $lines.Count; $i++) {
    if ($lines[$i] -match ('^## \[' + [regex]::Escape($v) + '\]')) { $start = $i; break }
}
if ($start -lt 0) { throw "CHANGELOG.md 里没有 $v 这一版：发版前先把「未发布」整理成 ## [$v] - 日期" }
$body = New-Object System.Collections.Generic.List[string]
for ($i = $start + 1; $i -lt $lines.Count; $i++) {
    # 到下一个版本、或文末的对比链接为止
    if ($lines[$i] -match '^## \[' -or $lines[$i] -match '^\[[^\]]+\]: ') { break }
    $body.Add($lines[$i])
}
$text = ($body -join "`n").Trim()
if (-not $text) { throw "CHANGELOG.md 里 $v 这一版是空的" }
$prev = ($lines | Select-String '^\[([^\]]+)\]: (\S+/compare/\S+)' |
    Where-Object { $_.Matches[0].Groups[1].Value -eq $v } | Select-Object -First 1)
if ($prev) { $text += "`n`n完整改动：$($prev.Matches[0].Groups[2].Value)" }
if ($Out) {
    [IO.File]::WriteAllText($Out, $text + "`n", [Text.UTF8Encoding]::new($false))
} else {
    $text
}
