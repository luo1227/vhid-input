#Requires -RunAsAdministrator
<#
.SYNOPSIS
  Install / uninstall the VhidInput test-signed driver on this machine.

.DESCRIPTION
  Test-signing only. Requires `bcdedit /set testsigning on` and a reboot
  if it is not already set. The catalog is signed with a self-signed test
  certificate that is added to the machine's Trusted Root and Trusted
  Publisher stores; remove both after uninstalling with -RemoveCert.

.PARAMETER Uninstall
  Remove the device and stop/delete the service.

.PARAMETER RemoveCert
  Also remove the self-signed test certificate from the machine stores.
#>
[CmdletBinding()]
param(
    [switch]$Uninstall,
    [switch]$RemoveCert
)

$ErrorActionPreference = 'Stop'

$root      = Split-Path -Parent $PSScriptRoot          # vhid-input\
$bin       = Join-Path $root 'bin'
$sys       = Join-Path $bin  'vhidinput.sys'
$infSrc    = Join-Path $root 'driver\vhidinput.inf'
$pkg       = Join-Path $bin  'pkg'
$cat       = Join-Path $pkg  'vhidinput.cat'
$certName  = 'VhidInput Test'
$certStoreRoot = 'Cert:\LocalMachine\Root'
$certStorePub  = 'Cert:\LocalMachine\TrustedPublisher'
$instanceId    = 'ROOT\VhidInput'

function Test-Testsigning {
    $line = (bcdedit /enum '{current}' 2>$null | Select-String 'testsigning\s+(\w+)')
    return ($line -and $line.Matches.Count -gt 0 -and $line.Matches[0].Groups[1].Value -eq 'Yes')
}

function Find-TestCert {
    @($certStoreRoot, $certStorePub) | ForEach-Object {
        Get-ChildItem $_ | Where-Object { $_.Subject -like "*CN=$certName*" }
    } | Select-Object -First 1
}

if ($Uninstall) {
    Write-Host '[install] removing device'
    $devcon = $(
        Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\Tools' -Recurse -Filter devcon.exe |
            Where-Object { $_.FullName -match 'x64' } |
            Sort-Object FullName -Descending |
            Select-Object -First 1
    ).FullName
    if ($devcon) { & $devcon remove 'Root\VhidInput' 2>&1 | Out-Host }
    Write-Host '[install] removing driver package'
    & pnputil.exe /delete-driver (Join-Path $pkg 'vhidinput.inf') /uninstall 2>&1 | Out-Host

    if ($RemoveCert) {
        Write-Host '[install] removing test certificate'
        Get-ChildItem $certStoreRoot | Where-Object { $_.Subject -like "*CN=$certName*" } |
            Remove-Item -ErrorAction SilentlyContinue
        Get-ChildItem $certStorePub  | Where-Object { $_.Subject -like "*CN=$certName*" } |
            Remove-Item -ErrorAction SilentlyContinue
    }
    Write-Host '[install] done. Reboot if the driver was in use.'
    exit 0
}

if (-not (Test-Path $sys)) { throw "Build first: $sys not found (run driver\mk.bat)." }
if (-not (Test-Path $infSrc)) { throw "INF not found: $infSrc" }
if (-not (Test-Testsigning)) {
    throw 'Test signing is off. Run: bcdedit /set testsigning on ; then reboot.'
}

$signtool = $(
    Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\bin' -Recurse -Filter signtool.exe |
        Where-Object { $_.FullName -match 'x64' } |
        Sort-Object FullName -Descending |
        Select-Object -First 1
).FullName
$inf2cat = $(
    Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\bin' -Recurse -Filter Inf2Cat.exe |
        Sort-Object FullName -Descending |
        Select-Object -First 1
).FullName
if (-not $signtool) { throw 'signtool.exe not found in the Windows Kits.' }

# ---- 1. stage the driver package ----
Write-Host "[install] staging package in $pkg"
New-Item -ItemType Directory -Force -Path $pkg | Out-Null
Copy-Item $infSrc $pkg -Force
Copy-Item $sys    $pkg -Force

# ---- 2. generate + sign the catalog ----
# pnputil accepts a signed catalog. Inf2Cat is the only way to produce one
# for a root-enumerated device, so run it then sign the .cat.
Write-Host '[install] building catalog'
& $inf2cat /driver:"$pkg" /os:10_X64,10_RS5_X64,10_VB_X64 /verbose 2>&1 |
    Select-Object -Last 5 | Out-Host
if (-not (Test-Path $cat)) { throw 'Inf2Cat did not produce vhidinput.cat' }

$cert = Find-TestCert
if (-not $cert) {
    Write-Host '[install] creating self-signed test certificate'
    $cert = New-SelfSignedCertificate `
        -Type Custom `
        -Subject "CN=$certName" `
        -KeyUsage DigitalSignature, CertSign `
        -KeyAlgorithm RSA -KeyLength 2048 `
        -HashAlgorithm SHA256 `
        -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3,1.3.6.1.5.5.7.3.8', '2.5.29.19={text}ca=0') `
        -CertStoreLocation 'Cert:\CurrentUser\My'
    # Import into the machine trust stores so the kernel loader trusts the
    # catalog. Same thumbprint, different stores.
    $tmp = Join-Path $env:TEMP 'vhidinput-test.cer'
    Export-Certificate -Cert $cert -FilePath $tmp | Out-Null
    Import-Certificate -FilePath $tmp -CertStoreLocation $certStoreRoot | Out-Null
    Import-Certificate -FilePath $tmp -CertStoreLocation $certStorePub  | Out-Null
    Remove-Item $tmp -Force
}

Write-Host '[install] signing catalog'
& $signtool sign /fd SHA256 /v /s My /n $certName $cat 2>&1 | Out-Host
if ($LASTEXITCODE -ne 0) { throw "signtool failed ($LASTEXITCODE)" }
& $signtool verify /pa /v $cat 2>&1 | Select-Object -Last 3 | Out-Host

# ---- 3. install via pnputil ----
# Exit codes that mean the package landed: 0 (imported), 3010 (imported, wants
# a reboot to swap the in-use .sys), and 259 (ERROR_NO_MORE_ITEMS: the package
# is already present at this DriverVer, because INF version numbers drive PnP
# and an unchanged DriverVer is silently "no update available"). Treating 259
# as failure aborted the script here, leaving the device disabled and the old
# .sys loaded -- which is exactly how a fresh build appeared to do nothing.
Write-Host '[install] adding driver package'
& pnputil.exe /add-driver (Join-Path $pkg 'vhidinput.inf') /install 2>&1 | Out-Host
$pnputilExit = $LASTEXITCODE
if ($pnputilExit -eq 259) {
    Write-Host '[install] package already at this DriverVer; device update below proceeds.'
} elseif ($pnputilExit -ne 0 -and $pnputilExit -ne 3010) {
    throw "pnputil /add-driver failed ($pnputilExit)"
} elseif ($pnputilExit -eq 3010) {
    Write-Host '[install] pnputil asked for a reboot to finish the driver swap.'
}

Write-Host '[install] creating root-enumerated device'
# This Windows build has no `pnputil /add-device`, so an absent ROOT node has
# to be created with devcon from the Windows Kits.
# Re-running the installer must update the existing node, not create another:
# every devcon "install" of the same instance id spawns a fresh ROOT\SYSTEM\NNNN
# node, and two live instances fight over the \\.\VhidInput DOS name (the second
# fails with STATUS_INVALID_DEVICE_REQUEST).
$installedVersion = (Get-Content (Join-Path $pkg 'vhidinput.inf') |
    Select-String -Pattern '^DriverVer\s*=\s*[^,]+,\s*(\S+)' |
    Select-Object -First 1).Matches.Groups[1].Value

$existing = Get-PnpDevice -PresentOnly -ErrorAction SilentlyContinue |
    Where-Object { $_.InstanceId -like 'ROOT\*' -and $_.FriendlyName -like '*VhidInput*' } |
    Select-Object -First 1

if (-not $existing) {
    $devcon = $(
        Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\Tools' -Recurse -Filter devcon.exe |
            Where-Object { $_.FullName -match 'x64' } |
            Sort-Object FullName -Descending |
            Select-Object -First 1
    ).FullName
    if (-not $devcon) { throw 'devcon.exe not found in the Windows Kits.' }
    Write-Host '[install] no device yet; creating ROOT\VhidInput'
    & $devcon install (Join-Path $pkg 'vhidinput.inf') 'Root\VhidInput' 2>&1 | Out-Host
} else {
    # pnputil /add-driver /install above already binds the newest package to the
    # existing node. Do NOT follow it with `devcon update`: on an up-to-date
    # device it fails with a bare "devcon.exe failed." and no usable reason, and
    # the only thing it would have done is re-apply what PnP just did.
    $bound = (Get-PnpDeviceProperty -InstanceId $existing.InstanceId `
        -KeyName 'DEVPKEY_Device_DriverVersion' -ErrorAction SilentlyContinue).Data
    Write-Host "[install] device $($existing.InstanceId) bound to driver version $bound (package $installedVersion)"
    if ($bound -ne $installedVersion) {
        Write-Warning ('The device is still on a different driver version than this package. ' +
                       'PnP only accepts a different version, so bump DriverVer in the INF and re-run.')
    }
}

# Report which image is actually staged in System32 versus what was built, so
# the next step (usually a reboot) is chosen with the real state in hand.
$sysInstalled = 'C:\Windows\System32\drivers\vhidinput.sys'
if ((Test-Path $sysInstalled) -and (Test-Path $sys)) {
    $a = (Get-FileHash $sys -Algorithm SHA256).Hash
    $b = (Get-FileHash $sysInstalled -Algorithm SHA256).Hash
    if ($a -eq $b) {
        Write-Host '[install] System32 image matches the build. The running copy only changes after a reboot.'
    } else {
        Write-Warning ('System32 holds a different image than this build. PnP did not swap it; ' +
                       'bump the INF DriverVer or copy it manually once the device is stopped.')
    }
}

Start-Sleep -Seconds 2
$svc = Get-Service vhidinput -ErrorAction SilentlyContinue
if ($svc) { $svc | Select-Object Name, Status | Format-Table | Out-Host }
$dev = Get-PnpDevice -InstanceId $instanceId -ErrorAction SilentlyContinue
if ($dev) { $dev | Select-Object Status, Class, FriendlyName | Format-Table | Out-Host }

Write-Host '[install] done. Test with: cargo run -p vhid-input --example selftest'
