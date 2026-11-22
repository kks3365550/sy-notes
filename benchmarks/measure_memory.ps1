# C:\Users\kksjmj\Desktop\ai\sy-notes\benchmarks\measure_memory.ps1
# WebView2 Process Tree WorkingSet Combined Memory Measurement Script

param (
    [string]$ProcessName = "sy-notes"
)

$mainProc = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $mainProc) {
    Write-Error "Process $ProcessName not found! Please ensure $ProcessName is running."
    exit 1
}

$mainPid = $mainProc.Id
Write-Host "Found main process: $ProcessName (PID: $mainPid)" -ForegroundColor Cyan

# Gather all processes in the system via CIM
$allCimProcs = Get-CimInstance Win32_Process

# Function to recursively find all descendant PIDs
function Get-ChildPids($parentPid, $allProcs) {
    $children = $allProcs | Where-Object { $_.ParentProcessId -eq $parentPid }
    $pids = @()
    foreach ($child in $children) {
        $pids += $child.ProcessId
        $pids += Get-ChildPids $child.ProcessId $allProcs
    }
    return $pids
}

$descendantPids = Get-ChildPids $mainPid $allCimProcs

# Also look for any msedgewebview2 instances associated with sy-notes / EBWebView data folder
$webviewProcs = $allCimProcs | Where-Object {
    $_.Name -like "*msedgewebview2*" -and ($_.CommandLine -like "*sy-notes*" -or $_.CommandLine -like "*EBWebView*")
}
$webviewPids = $webviewProcs | ForEach-Object { $_.ProcessId }

$allTargetPids = ($descendantPids + $webviewPids + $mainPid) | Select-Object -Unique

Write-Host "Target Process PIDs in Tree: $($allTargetPids -join ', ')" -ForegroundColor Yellow

$totalWS = 0
$totalPrivate = 0
$details = @()

foreach ($pidVal in $allTargetPids) {
    $p = Get-Process -Id $pidVal -ErrorAction SilentlyContinue
    if ($p) {
        $wsMB = [math]::Round($p.WorkingSet64 / 1MB, 2)
        $pmMB = [math]::Round($p.PrivateMemorySize64 / 1MB, 2)
        $totalWS += $p.WorkingSet64
        $totalPrivate += $p.PrivateMemorySize64
        $details += [PSCustomObject]@{
            PID = $p.Id
            ProcessName = $p.ProcessName
            WorkingSet_MB = $wsMB
            PrivateMemory_MB = $pmMB
        }
    }
}

$details | Format-Table -AutoSize

$totalWS_MB = [math]::Round($totalWS / 1MB, 2)
$totalPrivate_MB = [math]::Round($totalPrivate / 1MB, 2)

Write-Host "=================================================" -ForegroundColor Green
Write-Host "Total WebView2 Tree WorkingSet: $totalWS_MB MB" -ForegroundColor Green
Write-Host "Total Private Memory:          $totalPrivate_MB MB" -ForegroundColor Green
Write-Host "Target Threshold:              < 150.0 MB" -ForegroundColor Green

if ($totalWS_MB -lt 150.0) {
    Write-Host "[PASS] Memory usage ($totalWS_MB MB) is strictly below 150MB limit." -ForegroundColor Green
} else {
    Write-Host "[FAIL] Memory usage ($totalWS_MB MB) exceeds 150MB limit!" -ForegroundColor Red
}
Write-Host "=================================================" -ForegroundColor Green
