#Requires -RunAsAdministrator
<#
.SYNOPSIS
    Installs the test-signed Rust DisplayLink DL-1x5 UMDF/IddCx driver onto USB\VID_17E9&PID_04A3.
    Run this script from an Administrator PowerShell terminal.
#>
$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent $PSScriptRoot
$Dist = Join-Path $Root "dist"
$CerPath = Join-Path $Dist "cert\dl_rust_idd_test.cer"
$InfPath = Join-Path $Dist "dl_rust_idd.inf"

if (-not (Test-Path $InfPath)) {
    throw "Package not found at $Dist. Run .\scripts\package_and_sign.ps1 first."
}

Write-Host "[1/4] Checking Windows Test Signing mode..." -ForegroundColor Cyan
$bcd = bcdedit /enum "{current}" | Out-String
if ($bcd -notmatch "testsigning\s+Yes") {
    Write-Warning "Windows Test Signing is currently OFF."
    Write-Host "UMDF/Display test-signed drivers require Test Signing enabled (and Secure Boot disabled in BIOS if blocked)." -ForegroundColor Yellow
    Write-Host "Running: bcdedit /set testsigning on" -ForegroundColor Yellow
    bcdedit /set testsigning on
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to enable testsigning (Secure Boot may be enabled in UEFI). Disable Secure Boot or enable testsigning manually, then reboot."
    }
    Write-Warning "Test signing enabled! Please REBOOT Windows, then re-run this script."
    exit 0
}

Write-Host "[2/4] Trusting local test certificate in LocalMachine\Root and TrustedPublisher..." -ForegroundColor Cyan
Import-Certificate -FilePath $CerPath -CertStoreLocation "Cert:\LocalMachine\Root" | Out-Null
Import-Certificate -FilePath $CerPath -CertStoreLocation "Cert:\LocalMachine\TrustedPublisher" | Out-Null

Write-Host "[3/4] Adding driver package to Windows Driver Store..." -ForegroundColor Cyan
pnputil /add-driver "$InfPath" /install
if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne 259) {
    throw "pnputil /add-driver failed with exit code $LASTEXITCODE"
}

Write-Host "[4/4] Restarting and verifying USB\VID_17E9&PID_04A3..." -ForegroundColor Cyan
$dev = Get-PnpDevice | Where-Object { $_.InstanceId -match 'USB\\VID_17E9&PID_04A3' }
if ($dev) {
    pnputil /restart-device "$($dev.InstanceId)" | Out-Null
    Start-Sleep -Seconds 2
    $dev = Get-PnpDevice | Where-Object { $_.InstanceId -match 'USB\\VID_17E9&PID_04A3' }
}
$dev | Format-List Status, Class, FriendlyName, InstanceId, Manufacturer
Get-CimInstance Win32_PnPSignedDriver |
    Where-Object { $_.DeviceID -match 'VID_17E9&PID_04A3' } |
    Select-Object DeviceName, DriverVersion, InfName, Manufacturer | Format-List

if (Test-Path "C:\Users\Public\dl_rust_idd.log") {
    Write-Host "=== Driver Trace Log (C:\Users\Public\dl_rust_idd.log) ===" -ForegroundColor Yellow
    Get-Content "C:\Users\Public\dl_rust_idd.log" -Tail 25
}

Write-Host "Done! If Windows kept the old DisplayLink driver due to WHQL rank, open Device Manager -> Display adapters -> DTU-1031 -> Update driver -> Browse my computer -> Let me pick -> select 'DisplayLink DL-1x5 Rust Indirect Display (DTU-1031)'." -ForegroundColor Green
