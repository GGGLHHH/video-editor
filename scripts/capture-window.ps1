<#
.SYNOPSIS
  截取指定进程主窗口的画面（PrintWindow，窗口被遮挡也能截，不抢焦点）。
.EXAMPLE
  pwsh scripts/capture-window.ps1 -ProcessId 1234 -Out shot.png
#>
param(
    [Parameter(Mandatory)][int]$ProcessId,
    [Parameter(Mandatory)][string]$Out
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Win {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
}
"@
# 不声明 DPI 感知时，高缩放屏幕上拿到的是缩小后的坐标
[void][Win]::SetProcessDPIAware()
$p = Get-Process -Id $ProcessId
$h = $p.MainWindowHandle
if ($h -eq [IntPtr]::Zero) { throw "进程 $ProcessId 没有主窗口" }
$r = New-Object Win+RECT
[void][Win]::GetWindowRect($h, [ref]$r)
$w = $r.Right - $r.Left; $hgt = $r.Bottom - $r.Top
$bmp = New-Object System.Drawing.Bitmap $w, $hgt
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
# 2 = PW_RENDERFULLCONTENT：DirectComposition 窗口也能截
[void][Win]::PrintWindow($h, $hdc, 2)
$g.ReleaseHdc($hdc)
$bmp.Save($Out)
"$w x $hgt -> $Out"
