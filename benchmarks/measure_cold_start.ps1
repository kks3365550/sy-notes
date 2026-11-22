# C:\Users\kksjmj\Desktop\ai\sy-notes\benchmarks\measure_cold_start.ps1
# Cold Start Time Measurement Script (< 1.0s window display)

param (
    [string]$ExePath = "C:\Users\kksjmj\Desktop\ai\sy-notes\src-tauri\target\release\sy-notes.exe"
)

# Win32 API to check window visibility
$user32Code = @"
using System;
using System.Runtime.InteropServices;

public class Win32 {
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool IsWindowVisible(IntPtr hWnd);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }
}
"@

Add-Type -TypeDefinition $user32Code -ErrorAction SilentlyContinue

if (-not (Test-Path $ExePath)) {
    # Check debug build fallback
    $debugPath = "C:\Users\kksjmj\Desktop\ai\sy-notes\src-tauri\target\debug\sy-notes.exe"
    if (Test-Path $debugPath) {
        Write-Host "Release binary not found, using debug binary: $debugPath" -ForegroundColor Yellow
        $ExePath = $debugPath
    } else {
        Write-Error "Executable not found at $ExePath or $debugPath. Please build first."
        exit 1
    }
}

Write-Host "Target Executable: $ExePath" -ForegroundColor Cyan

# Ensure clean slate: kill any running instances
Get-Process -Name "sy-notes" -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 500

Write-Host "Starting cold start measurement..." -ForegroundColor Cyan
$sw = [System.Diagnostics.Stopwatch]::StartNew()

$proc = Start-Process -FilePath $ExePath -PassThru
$windowShown = $false
$elapsedMs = 0

while ($sw.ElapsedMilliseconds -lt 10000) {
    $proc.Refresh()
    $hWnd = $proc.MainWindowHandle
    if ($hWnd -ne [IntPtr]::Zero) {
        $visible = [Win32]::IsWindowVisible($hWnd)
        $rect = New-Object Win32+RECT
        [Win32]::GetWindowRect($hWnd, [ref]$rect)
        $width = $rect.Right - $rect.Left
        $height = $rect.Bottom - $rect.Top
        if ($visible -and $width -gt 100 -and $height -gt 100) {
            $sw.Stop()
            $elapsedMs = $sw.ElapsedMilliseconds
            $windowShown = $true
            break
        }
    }
    Start-Sleep -Milliseconds 10
}

Write-Host "=================================================" -ForegroundColor Green
if ($windowShown) {
    Write-Host "Cold Start Elapsed Time: $elapsedMs ms ($([math]::Round($elapsedMs / 1000, 3)) s)" -ForegroundColor Green
    Write-Host "Threshold Target:        < 1000 ms (< 1.0s)" -ForegroundColor Green
    if ($elapsedMs -lt 1000) {
        Write-Host "[PASS] Cold start is under 1.0s threshold!" -ForegroundColor Green
    } else {
        Write-Host "[FAIL] Cold start ($elapsedMs ms) exceeded 1000ms threshold." -ForegroundColor Red
    }
} else {
    Write-Host "[FAIL] Window was not shown within 10 seconds!" -ForegroundColor Red
}
Write-Host "=================================================" -ForegroundColor Green

# Clean up launched process
if ($proc -and -not $proc.HasExited) {
    $proc.Kill()
}
