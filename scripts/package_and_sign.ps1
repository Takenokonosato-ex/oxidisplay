#Requires -Version 5.1
<#
.SYNOPSIS
    Builds dl_rust_idd.dll in release mode, creates dl_rust_idd.cat via WDK Inf2Cat,
    and test-signs both the DLL and CAT using a local code-signing certificate.
#>
$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent $PSScriptRoot
$Dist = Join-Path $Root "dist"
$CertDir = Join-Path $Root "dist\cert"

Write-Host "[1/5] Building Rust workspace (release)..." -ForegroundColor Cyan
Push-Location $Root
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build --release failed" }
} finally {
    Pop-Location
}

Write-Host "[2/5] Staging driver package in $Dist..." -ForegroundColor Cyan
New-Item -ItemType Directory -Force -Path $Dist | Out-Null
New-Item -ItemType Directory -Force -Path $CertDir | Out-Null

$DllSrc = Join-Path $Root "target\release\dl_rust_idd.dll"
$InfSrc = Join-Path $Root "driver\dl_rust_idd.inf"
Copy-Item -Force $DllSrc (Join-Path $Dist "dl_rust_idd.dll")
Copy-Item -Force $InfSrc (Join-Path $Dist "dl_rust_idd.inf")

# Locate WDK tools
$KitsBin = "C:\Program Files (x86)\Windows Kits\10\bin"
$Inf2Cat = Get-ChildItem $KitsBin -Recurse -Filter "Inf2Cat.exe" -ErrorAction SilentlyContinue |
    Sort-Object FullName -Descending | Select-Object -First 1 -ExpandProperty FullName
$SignTool = Get-ChildItem $KitsBin -Recurse -Filter "signtool.exe" -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match '\\x64\\' } |
    Sort-Object FullName -Descending | Select-Object -First 1 -ExpandProperty FullName

if (-not $Inf2Cat) { throw "Inf2Cat.exe not found under $KitsBin" }
if (-not $SignTool) { throw "signtool.exe not found under $KitsBin" }

Write-Host "[3/5] Creating catalog (dl_rust_idd.cat) with Inf2Cat..." -ForegroundColor Cyan
& $Inf2Cat /driver:"$Dist" /os:10_GE_X64 /uselocaltime
if ($LASTEXITCODE -ne 0) {
    # Fallback to Windows 10 x64 OS target if 10_GE_X64 is not supported by older Inf2Cat
    & $Inf2Cat /driver:"$Dist" /os:10_X64 /uselocaltime
    if ($LASTEXITCODE -ne 0) { throw "Inf2Cat failed" }
}

Write-Host "[4/5] Preparing local test-signing certificate..." -ForegroundColor Cyan
$CertSubject = "CN=Rust DisplayLink DL-1x5 Test Driver"
$Cert = Get-ChildItem Cert:\CurrentUser\My | Where-Object { $_.Subject -eq $CertSubject } | Select-Object -First 1
if (-not $Cert) {
    $Cert = New-SelfSignedCertificate `
        -Type CodeSigningCert `
        -Subject $CertSubject `
        -CertStoreLocation "Cert:\CurrentUser\My" `
        -KeyExportPolicy Exportable `
        -KeyLength 2048 `
        -HashAlgorithm SHA256 `
        -NotAfter (Get-Date).AddYears(5)
}

$CerPath = Join-Path $CertDir "dl_rust_idd_test.cer"
Export-Certificate -Cert $Cert -FilePath $CerPath -Force | Out-Null

Write-Host "[5/5] Test-signing dl_rust_idd.dll and dl_rust_idd.cat..." -ForegroundColor Cyan
$Thumb = $Cert.Thumbprint
& $SignTool sign /v /s My /sha1 $Thumb /fd SHA256 (Join-Path $Dist "dl_rust_idd.dll")
if ($LASTEXITCODE -ne 0) { throw "signtool failed on dl_rust_idd.dll" }
& $SignTool sign /v /s My /sha1 $Thumb /fd SHA256 (Join-Path $Dist "dl_rust_idd.cat")
if ($LASTEXITCODE -ne 0) { throw "signtool failed on dl_rust_idd.cat" }

Write-Host "`nDriver package ready in: $Dist" -ForegroundColor Green
Get-ChildItem $Dist | Format-Table Name, Length, LastWriteTime -AutoSize
