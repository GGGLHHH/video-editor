<#
.SYNOPSIS
  发一个新版本：just release 0.3.0（或 pwsh scripts/release.ps1 -Version 0.3.0）。
.DESCRIPTION
  发版前先把 CHANGELOG.md 的「## [未发布]」写好。脚本会：
    1. 检查：在 main 上、没有未提交的改动、和远端一致、版本号比现在大、标签还没有、「未发布」不是空的；
    2. 把「未发布」改成「## [x.y.z] - 今天」（上面留一个新的空「未发布」），补好文末的对比链接，
       改 Cargo.toml 的版本号（Cargo.lock 跟着变）；
    3. 问一句，确认后：开分支 chore/release-x.y.z 提交、推送、开 PR，等 CI 通过后压缩合并；
    4. 在 main 上打 vx.y.z 标签推上去，触发 Release 工作流（全量检查 → 打包 → 建 GitHub Release），
       等它跑完，打印 Release 地址。
  -DryRun 只打印会改成什么，不改文件、不碰 git。-Yes 不问直接推。
.EXAMPLE
  pwsh scripts/release.ps1 -Version 0.3.0 -DryRun
#>
param(
    [Parameter(Mandatory)] [string]$Version,
    [switch]$DryRun,
    [switch]$Yes
)
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')
$utf8 = [Text.UTF8Encoding]::new($false)

function Write-Step([string]$Text) { Write-Host "==> $Text" -ForegroundColor Cyan }
function Stop-Release([string]$Why) { Write-Host "停下：$Why" -ForegroundColor Red; exit 1 }
function Invoke-Native {
    & $args[0] @($args | Select-Object -Skip 1)
    if ($LASTEXITCODE -ne 0) { Stop-Release "$($args -join ' ') 失败（退出码 $LASTEXITCODE）" }
}

# ---------- 1. 检查 ----------
$v = $Version.TrimStart('v')
if ($v -notmatch '^\d+\.\d+\.\d+$') { Stop-Release "版本号要写成 1.2.3 这样：$Version" }
$tag = "v$v"

$cargoText = [IO.File]::ReadAllText((Resolve-Path Cargo.toml), $utf8)
$current = [regex]::Match($cargoText, '(?m)^version = "([^"]+)"').Groups[1].Value
if (-not $current) { Stop-Release 'Cargo.toml 里找不到 version' }
if ([version]$v -le [version]$current) { Stop-Release "新版本 $v 要比现在的 $current 大" }

$changelogPath = (Resolve-Path CHANGELOG.md).Path
$lines = [IO.File]::ReadAllText($changelogPath, $utf8) -split "`r?`n"
$head = [Array]::IndexOf($lines, '## [未发布]')
if ($head -lt 0) { Stop-Release 'CHANGELOG.md 里找不到「## [未发布]」' }
$next = $head + 1
while ($next -lt $lines.Count -and $lines[$next] -notmatch '^## \[' -and $lines[$next] -notmatch '^\[[^\]]+\]: ') { $next++ }
$unreleased = ($lines[($head + 1)..($next - 1)] -join "`n").Trim()
if (-not $unreleased) { Stop-Release 'CHANGELOG.md 的「未发布」是空的：先写这一版改了什么' }

$linkIdx = -1
for ($i = 0; $i -lt $lines.Count; $i++) { if ($lines[$i] -match '^\[未发布\]: (.+)/compare/(v[^.]+\.[^.]+\.[^.]+)\.\.\.HEAD$') { $linkIdx = $i; break } }
if ($linkIdx -lt 0) { Stop-Release 'CHANGELOG.md 文末找不到「[未发布]: …/compare/vX.Y.Z...HEAD」链接' }
$repoUrl = $Matches[1]
$prevTag = $Matches[2]

if (-not $DryRun) {
    Write-Step '检查 git 状态'
    $branch = (git branch --show-current).Trim()
    if ($branch -ne 'main') { Stop-Release "要在 main 上发版（现在在 $branch）" }
    if (git status --porcelain) { Stop-Release '有没提交的改动：先提交或收起来' }
    Invoke-Native git fetch --quiet origin main --tags
    if ((git rev-parse HEAD) -ne (git rev-parse origin/main)) { Stop-Release '本地 main 和远端不一致：先 git pull' }
    git rev-parse -q --verify "refs/tags/$tag" *> $null
    if ($LASTEXITCODE -eq 0) { Stop-Release "标签 $tag 已经有了" }
    gh auth status *> $null
    if ($LASTEXITCODE -ne 0) { Stop-Release 'gh 没登录：先 gh auth login' }
}

# ---------- 2. 改文件 ----------
$date = Get-Date -Format 'yyyy-MM-dd'
$newLines = [System.Collections.Generic.List[string]]::new()
for ($i = 0; $i -lt $lines.Count; $i++) {
    if ($i -eq $head) {
        $newLines.Add('## [未发布]'); $newLines.Add(''); $newLines.Add("## [$v] - $date")
    } elseif ($i -eq $linkIdx) {
        $newLines.Add("[未发布]: $repoUrl/compare/$tag...HEAD")
        $newLines.Add("[$v]: $repoUrl/compare/$prevTag...$tag")
    } else {
        $newLines.Add($lines[$i])
    }
}
$newChangelog = $newLines -join "`n"
$newCargo = [regex]::new('(?m)^version = "[^"]+"').Replace($cargoText, "version = `"$v`"", 1)

Write-Host ''
Write-Host "版本 $current → $v，标签 $tag，日期 $date" -ForegroundColor Yellow
Write-Host "---- 发版说明（$tag）----"
Write-Host $unreleased
Write-Host "完整改动：$repoUrl/compare/$prevTag...$tag"
Write-Host '-------------------------'
if ($DryRun) {
    Write-Host '（-DryRun：没改文件、没碰 git）' -ForegroundColor Yellow
    exit 0
}

Write-Step '改 CHANGELOG.md、Cargo.toml、Cargo.lock'
[IO.File]::WriteAllText($changelogPath, $newChangelog, $utf8)
[IO.File]::WriteAllText((Resolve-Path Cargo.toml).Path, $newCargo, $utf8)
Invoke-Native cargo update --workspace --quiet
$check = pwsh -NoProfile -File scripts/release-notes.ps1 -Version $v
if ($LASTEXITCODE -ne 0) { Stop-Release '改完的 CHANGELOG.md 取不出这一版的说明' }

# ---------- 3. PR ----------
if (-not $Yes) {
    Write-Host ''
    Write-Host "接下来会：开分支 chore/release-$v 提交并推送、开 PR、等 CI 通过后压缩合并，" -ForegroundColor Yellow
    Write-Host "再打标签 $tag 推上去触发发版（GitHub 上的 Release 是公开的）。" -ForegroundColor Yellow
    $answer = Read-Host '继续吗？输入 y 继续，其他任何键取消（取消后改动留在工作区）'
    if ($answer -ne 'y') { Stop-Release '取消了：文件已经改好，没有提交（git diff 可以看，git checkout . 可以撤销）' }
}
$branchName = "chore/release-$v"
Write-Step "提交到 $branchName 并推送"
Invoke-Native git switch --quiet -c $branchName
Invoke-Native git add CHANGELOG.md Cargo.toml Cargo.lock
Invoke-Native git commit --quiet -m "chore(release): $v"
Invoke-Native git push --quiet -u origin $branchName
$notesFile = Join-Path ([IO.Path]::GetTempPath()) "release-$v.md"
[IO.File]::WriteAllText($notesFile, ($check -join "`n"), $utf8)
Invoke-Native gh pr create --base main --head $branchName --title "chore(release): $v" --body-file $notesFile

Write-Step '等 PR 的 CI（全量检查大约 30 分钟）'
# 刚开的 PR 要等一会儿才有检查项
for ($i = 0; $i -lt 30; $i++) {
    if (gh pr checks $branchName 2>$null) { break }
    Start-Sleep -Seconds 10
}
gh pr checks $branchName --watch --interval 60
if ($LASTEXITCODE -ne 0) { Stop-Release "CI 没通过：PR 留着（gh pr view $branchName --web 看原因），修好推上去后手动合并，再在 main 上打 $tag" }

Write-Step '压缩合并 PR'
Invoke-Native gh pr merge $branchName --squash --delete-branch --subject "chore(release): $v"
Invoke-Native git switch --quiet main
Invoke-Native git pull --quiet --ff-only
if (-not (Select-String -Path Cargo.toml -Pattern "^version = `"$([regex]::Escape($v))`"" -Quiet)) { Stop-Release "合并后 main 上的版本号不是 $v" }

# ---------- 4. 标签和发版 ----------
Write-Step "打标签 $tag 并推送"
Invoke-Native git tag -a $tag -m "视频剪辑器 $tag"
Invoke-Native git push --quiet origin $tag

Write-Step '等 Release 工作流（全量检查 + 打包，大约一小时）'
$runId = $null
for ($i = 0; $i -lt 30 -and -not $runId; $i++) {
    Start-Sleep -Seconds 10
    $runId = gh run list --workflow release.yml --branch $tag --limit 1 --json databaseId -q '.[0].databaseId'
}
if (-not $runId) { Stop-Release "找不到 $tag 的 Release 工作流：去 Actions 页面看看" }
gh run watch $runId --interval 60 --exit-status
if ($LASTEXITCODE -ne 0) { Stop-Release "Release 工作流失败：gh run view $runId --log-failed 看原因" }
$url = gh release view $tag --json url -q .url
Write-Host ''
Write-Host "发好了：$url" -ForegroundColor Green
