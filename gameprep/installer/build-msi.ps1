#Requires -Version 5.1
<#
.SYNOPSIS
    Builds dist\GamePrep-<version>-x64.msi (and a copy of the .exe).

.DESCRIPTION
    No admin needed. Requires the Rust MSVC toolchain and the WiX Toolset
    v7 CLI (wix.exe). Using WiX v7 means accepting its OSMF EULA
    (-acceptEula wix7); the maintenance fee only applies to organizations
    with more than USD 10k/year revenue (docs.firegiant.com/wix/osmf).

    Keep this file ASCII-only (Windows PowerShell 5.1 + no BOM).
#>
[CmdletBinding()]
param(
    [string]$RepoRoot
)

$ErrorActionPreference = 'Stop'
if (-not $RepoRoot) {
    $RepoRoot = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path))
}
$installer = Join-Path $RepoRoot 'gameprep\installer'
$dist = Join-Path $RepoRoot 'dist'

$wix = Get-Command wix -ErrorAction SilentlyContinue
$wixPath = if ($wix) { $wix.Source } else {
    (Get-ChildItem "$env:ProgramFiles\WiX Toolset v7*\bin\wix.exe" -ErrorAction SilentlyContinue | Select-Object -First 1).FullName
}
if (-not $wixPath) { throw 'wix.exe (WiX Toolset v7) not found - install wix-cli-x64.msi from github.com/wixtoolset/wix/releases' }

$cargoToml = Get-Content (Join-Path $RepoRoot 'gameprep\Cargo.toml') -Raw
if ($cargoToml -notmatch '(?m)^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"') { throw 'version not found in gameprep/Cargo.toml' }
$version = $Matches[1]

$cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
if (-not (Test-Path $cargo)) { $cargo = 'cargo' }
Push-Location $RepoRoot
try {
    & $cargo build --release -p gameprep
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
} finally { Pop-Location }

New-Item -ItemType Directory -Force $dist | Out-Null
$exe = Join-Path $RepoRoot 'target\release\gameprep.exe'
$ico = Join-Path $dist 'gameprep.ico'
# Written to %TEMP% first: AV ransomware shields (Avast here) block a
# freshly built, unsigned exe from writing under Desktop/Documents.
$tmpIco = Join-Path $env:TEMP 'gameprep-build.ico'
Remove-Item $tmpIco -ErrorAction SilentlyContinue
# Start-Process -Wait, not `&`: the release exe is a GUI-subsystem app,
# and PowerShell doesn't wait for those when called with `&`.
$p = Start-Process -FilePath $exe -ArgumentList '--write-icon', "`"$tmpIco`"" -Wait -PassThru
if ($p.ExitCode -ne 0 -or -not (Test-Path $tmpIco)) { throw "icon not written (exit $($p.ExitCode))" }
Copy-Item $tmpIco $ico -Force
Copy-Item $exe (Join-Path $dist 'gameprep.exe') -Force

$msi = Join-Path $dist ("GamePrep-$version-x64.msi")
& $wixPath build (Join-Path $installer 'gameprep.wxs') `
    -arch x64 `
    -acceptEula wix7 `
    -d "Version=$version" `
    -d "ExePath=$exe" `
    -d "IconPath=$ico" `
    -o $msi
if ($LASTEXITCODE -ne 0) { throw 'wix build failed' }

# wix leaves a .wixpdb (debug symbols for the installer) next to the msi.
Remove-Item (Join-Path $dist '*.wixpdb') -ErrorAction SilentlyContinue

foreach ($f in (Join-Path $dist 'gameprep.exe'), $msi) {
    $h = (Get-FileHash $f -Algorithm SHA256).Hash
    '{0,-28} {1,8:N1} MB  SHA256 {2}' -f (Split-Path $f -Leaf), ((Get-Item $f).Length / 1MB), $h
}
