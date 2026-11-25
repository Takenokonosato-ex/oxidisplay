#Requires -RunAsAdministrator
<#
.SYNOPSIS
    Removes the Rust DisplayLink test driver and restores the original DisplayLink driver (DriverStore).
#>
$ErrorActionPreference = "Stop"

Write-Host "[1/2] Removing dl_rust_idd.inf from Driver Store (if installed)..." -ForegroundColor Cyan
$drivers = pnputil /enum-drivers | Out-String
$blocks = $drivers -split "\r?\n\r?\n"
foreach ($b in $blocks) {
    if ($b -match "dl_rust_idd\.inf" -and $b -match "Published Name\s*:\s*(oem\d+\.inf)") {
        $oem = $Matches[1]
        Write-Host "Deleting $oem..." -ForegroundColor Yellow
        pnputil /delete-driver $oem /uninstall /force
    }
}

Write-Host "[2/2] Triggering PnP device rescan to restore original DisplayLink driver..." -ForegroundColor Cyan
pnputil /scan-devices
Get-CimInstance Win32_PnPSignedDriver |
    Where-Object { $_.DeviceID -match 'VID_17E9&PID_04A3' } |
    Select-Object DeviceName, DriverVersion, InfName, Manufacturer | Format-List

Write-Host "Rollback complete." -ForegroundColor Green
