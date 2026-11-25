#Requires -RunAsAdministrator
<#
.SYNOPSIS
    Gold-standard A/B benchmark on the live Wacom DTU-1031 (1280x800 @ 60Hz)
    comparing Rust dl_rust_idd v0.2.0.0 vs Official DisplayLink dlidusb4 11.2.3146.0.
    Captures:
      - Exact real delivered IddCx frame rate via Windows ETW (Microsoft-Windows-IndirectDisplays-ClassExtension-Events)
      - Exact real USB throughput via WUDFHost IO OtherTransferCount (WinUSB DeviceIoControl bytes/sec)
      - Exact WUDFHost CPU %, WorkingSet, PrivateMemory, Memory Delta, Handle Count, DLL size
#>
$ErrorActionPreference = "Stop"

Add-Type -AssemblyName System.Windows.Forms
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;

[StructLayout(LayoutKind.Sequential)]
public struct IO_COUNTERS {
    public ulong ReadOperationCount;
    public ulong WriteOperationCount;
    public ulong OtherOperationCount;
    public ulong ReadTransferCount;
    public ulong WriteTransferCount;
    public ulong OtherTransferCount;
}

public static class ProcIo {
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool GetProcessIoCounters(IntPtr hProcess, out IO_COUNTERS lpIoCounters);
}
"@

$Root = Split-Path -Parent $PSScriptRoot
$BenchExe = Join-Path $Root "target\release\dl-bench.exe"
$RustInf = Join-Path $Root "dist\dl_rust_idd.inf"
$ReportPath = Join-Path $Root "dist\ab_benchmark_report.txt"
$IddEtwGuid = "{966CD1C0-3F69-42AD-9877-517DCE8462B4}"

function Find-DtuScreen {
    for ($i = 0; $i -lt 15; $i++) {
        $scr = [System.Windows.Forms.Screen]::AllScreens | Where-Object {
            $_.Bounds.Width -eq 1280 -and $_.Bounds.Height -eq 800
        } | Select-Object -First 1
        if ($scr) { return $scr }
        Start-Sleep -Seconds 1
    }
    throw "Could not find 1280x800 DTU-1031 monitor!"
}

function Find-DisplayLinkWudfHost {
    for ($i = 0; $i -lt 15; $i++) {
        foreach ($p in (Get-Process WUDFHost -ErrorAction SilentlyContinue)) {
            try {
                $m = $p.Modules | Where-Object { $_.ModuleName -match 'dl_rust_idd|dlidusb' } | Select-Object -First 1
                if ($m) {
                    return @{ Process = $p; Module = $m.ModuleName; FileName = $m.FileName }
                }
            } catch {}
        }
        Start-Sleep -Milliseconds 500
    }
    throw "Could not locate WUDFHost hosting DisplayLink driver!"
}

function Summarize-IddEtw([string]$EtlPath, [int]$TargetPid, [double]$ActiveSeconds) {
    if (-not (Test-Path $EtlPath)) { return "ETW file missing" }
    $events = Get-WinEvent -Path $EtlPath -Oldest -ErrorAction SilentlyContinue | Where-Object { $_.ProcessId -eq $TargetPid }
    if (-not $events) { return "ETW: 0 events for PID $TargetPid" }
    $groups = $events | Group-Object Id | Sort-Object Count -Descending
    $summary = ($groups | Select-Object -First 6 | ForEach-Object { "ID$($_.Name)=$($_.Count)" }) -join ", "
    # Find the highest-frequency frame-event ID (ReleaseAndAcquire / FinishedProcessingFrame)
    $topId = ($groups | Select-Object -First 1).Name
    $topEvs = $events | Where-Object { $_.Id -eq $topId }
    $frameTimes = New-Object System.Collections.Generic.List[double]
    for ($i = 1; $i -lt $topEvs.Count; $i++) {
        $dt = ($topEvs[$i].TimeCreated - $topEvs[$i-1].TimeCreated).TotalMilliseconds
        if ($dt -gt 0.1 -and $dt -lt 500.0) { $frameTimes.Add($dt) }
    }
    $fpsStr = ""
    if ($frameTimes.Count -gt 10) {
        $sorted = $frameTimes | Sort-Object
        $n = $sorted.Count
        $p50 = $sorted[[int]($n * 0.50)]
        $p95 = $sorted[[int]($n * 0.95)]
        $p99 = $sorted[[int]($n * 0.99)]
        $max = $sorted[$n - 1]
        $avgMs = ($frameTimes | Measure-Object -Average).Average
        $realFps = if ($ActiveSeconds -gt 0) { $topEvs.Count / $ActiveSeconds } else { 0 }
        $fpsStr = (" | RealIddCx(ID{0}): count={1}, avg_fps={2:F1} (active {3:F1}s), avg={4:F2}ms, p50={5:F2}ms, p95={6:F2}ms, p99={7:F2}ms, max={8:F2}ms" -f $topId, $topEvs.Count, $realFps, $ActiveSeconds, $avgMs, $p50, $p95, $p99, $max)
    }
    return "ETW Events (PID $TargetPid): [$summary]$fpsStr"
}

function Measure-DriverRun([string]$Label, [string]$Tag) {
    $scr = Find-DtuScreen
    $hostInfo = Find-DisplayLinkWudfHost
    $proc = $hostInfo.Process
    $dllSizeKB = [math]::Round((Get-Item $hostInfo.FileName).Length / 1KB, 1)
    $ver = (Get-Item $hostInfo.FileName).VersionInfo.FileVersion
    $cores = [Environment]::ProcessorCount

    $etlPath = Join-Path $Root "dist\etw_$Tag.etl"
    Remove-Item $etlPath -ErrorAction SilentlyContinue
    logman stop DlBenchEtw -ets 2>$null | Out-Null
    logman start DlBenchEtw -p $IddEtwGuid 0xffffffffffffffff 5 -o "$etlPath" -ets | Out-Null

    $proc.Refresh()
    $io0 = New-Object IO_COUNTERS
    [ProcIo]::GetProcessIoCounters($proc.Handle, [ref]$io0) | Out-Null
    $cpu0 = $proc.TotalProcessorTime
    $ws0 = $proc.WorkingSet64
    $priv0 = $proc.PrivateMemorySize64
    $hdl0 = $proc.HandleCount
    $t0 = Get-Date

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $BenchExe
    $psi.Arguments = "$($scr.Bounds.X) $($scr.Bounds.Y) $($scr.Bounds.Width) $($scr.Bounds.Height) 5"
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $benchProc = [System.Diagnostics.Process]::Start($psi)

    $peakCpuPct = 0.0
    $peakWs = $ws0
    $peakPriv = $priv0
    $prevCpu = $cpu0
    $prevTime = $t0
    while (-not $benchProc.HasExited) {
        Start-Sleep -Milliseconds 500
        try {
            $proc.Refresh()
            $now = Get-Date
            $curCpu = $proc.TotalProcessorTime
            $dt = ($now - $prevTime).TotalSeconds
            if ($dt -gt 0.05) {
                $sampleCpu = (($curCpu - $prevCpu).TotalSeconds / ($dt * $cores)) * 100.0
                if ($sampleCpu -gt $peakCpuPct) { $peakCpuPct = $sampleCpu }
            }
            if ($proc.WorkingSet64 -gt $peakWs) { $peakWs = $proc.WorkingSet64 }
            if ($proc.PrivateMemorySize64 -gt $peakPriv) { $peakPriv = $proc.PrivateMemorySize64 }
            $prevCpu = $curCpu
            $prevTime = $now
        } catch {}
    }

    $benchOut = $benchProc.StandardOutput.ReadToEnd()
    $benchProc.WaitForExit()

    $proc.Refresh()
    $t1 = Get-Date
    $io1 = New-Object IO_COUNTERS
    [ProcIo]::GetProcessIoCounters($proc.Handle, [ref]$io1) | Out-Null
    $cpu1 = $proc.TotalProcessorTime
    $ws1 = $proc.WorkingSet64
    $priv1 = $proc.PrivateMemorySize64
    $hdl1 = $proc.HandleCount

    logman stop DlBenchEtw -ets 2>$null | Out-Null

    $totalSec = ($t1 - $t0).TotalSeconds
    $activeSec = [math]::Max(1.0, $totalSec - 3.0) # exclude 3s idle phase
    $avgCpuTotal = (($cpu1 - $cpu0).TotalSeconds / ($totalSec * $cores)) * 100.0
    $avgCpuOneCore = (($cpu1 - $cpu0).TotalSeconds / $totalSec) * 100.0
    $usbOtherMB = ($io1.OtherTransferCount - $io0.OtherTransferCount) / 1MB
    $usbOtherMBps = $usbOtherMB / $activeSec
    $usbOtherOps = ($io1.OtherOperationCount - $io0.OtherOperationCount) / $activeSec
    $etwLine = Summarize-IddEtw $etlPath $proc.Id $activeSec

    $lines = @()
    $lines += "================================================================"
    $lines += "BENCHMARK: $Label"
    $lines += "  DLL:        $($hostInfo.Module) ($dllSizeKB KB on disk, ver=$ver)"
    $lines += "  Screen:     $($scr.DeviceName) $($scr.Bounds.X),$($scr.Bounds.Y) $($scr.Bounds.Width)x$($scr.Bounds.Height)"
    $lines += "  WUDFHost:   PID=$($proc.Id)"
    $lines += ("  CPU Usage:  Avg={0:F2}% (1-core eq: {1:F1}%) | Peak={2:F2}% ({3} logical CPUs)" -f $avgCpuTotal, $avgCpuOneCore, $peakCpuPct, $cores)
    $lines += ("  WorkingSet: Start={0:F1} MB -> End={1:F1} MB (Peak={2:F1} MB, Delta={3:+0.0;-0.0;0.0} MB)" -f ($ws0/1MB), ($ws1/1MB), ($peakWs/1MB), (($ws1-$ws0)/1MB))
    $lines += ("  PrivateMem: Start={0:F1} MB -> End={1:F1} MB (Peak={2:F1} MB, Delta={3:+0.0;-0.0;0.0} MB)" -f ($priv0/1MB), ($priv1/1MB), ($peakPriv/1MB), (($priv1-$priv0)/1MB))
    $lines += ("  Handles:    Start={0} -> End={1} (Delta={2:+0;-0;0})" -f $hdl0, $hdl1, ($hdl1 - $hdl0))
    $lines += ("  USB IOCTL:  Total={0:F1} MB | ActiveAvg={1:F2} MB/s | IOCTLs={2:F0} ops/s" -f $usbOtherMB, $usbOtherMBps, $usbOtherOps)
    $lines += "  $etwLine"
    $lines += "  --- Client 60Hz Workload Phases ---"
    foreach ($l in ($benchOut -split "`r?`n")) {
        if ($l.Trim()) { $lines += "  $l" }
    }
    return $lines
}

$allLines = @()

Write-Host "[1/4] Benchmarking Rust Driver (v0.2.0.0)..." -ForegroundColor Cyan
$allLines += Measure-DriverRun "Rust Driver (dl_rust_idd v0.2.0.0)" "rust"

Write-Host "[2/4] Switching to Official DisplayLink Driver (DriverStore)..." -ForegroundColor Yellow
$drivers = pnputil /enum-drivers | Out-String
foreach ($b in ($drivers -split "\r?\n\r?\n")) {
    if ($b -match "dl_rust_idd\.inf" -and $b -match "Published Name\s*:\s*(oem\d+\.inf)") {
        pnputil /delete-driver $Matches[1] /uninstall /force | Out-Null
    }
}
pnputil /scan-devices | Out-Null
Start-Sleep -Seconds 5

Write-Host "[3/4] Benchmarking Official DisplayLink Driver (11.2.3146.0)..." -ForegroundColor Cyan
$allLines += Measure-DriverRun "Official DisplayLink Driver (dlidusb4 11.2.3146.0)" "official"

Write-Host "[4/4] Restoring Rust Driver (v0.2.0.0)..." -ForegroundColor Green
pnputil /add-driver "$RustInf" /install | Out-Null
$dev = Get-PnpDevice | Where-Object { $_.InstanceId -match 'USB\\VID_17E9&PID_04A3' } | Select-Object -First 1
if ($dev) {
    pnputil /restart-device "$($dev.InstanceId)" | Out-Null
}
Start-Sleep -Seconds 3
$restoredHost = Find-DisplayLinkWudfHost
$allLines += "================================================================"
$allLines += "RESTORED ACTIVE MODULE: $($restoredHost.Module) (PID $($restoredHost.Process.Id))"

$allLines | Set-Content -Path $ReportPath -Encoding UTF8
$allLines | ForEach-Object { Write-Host $_ }
