# Fetch a portable NSIS toolchain into the repo tool cache (.cache\nsis).
#
# NSIS has no user-scope installer, so we download the official portable zip
# from SourceForge, verify its SHA-256 and unpack it. The cache directory is
# gitignored and is the same one CI links to its persistent runner cache, so a
# second run is a no-op.
#
# Result: .cache\nsis\nsis-<version>\makensis.exe

[CmdletBinding()]
param(
    # NSIS release to fetch. Keep -Sha256 in sync when bumping this.
    [string]$Version = "3.12",

    # SHA-256 of nsis-<version>.zip as published on SourceForge. Pass "" to skip
    # verification (not recommended).
    [string]$Sha256 = "56581f90db321581c5381193d796fffcf2d24b2f8fed2160a6c6a3baa67f2c4f",

    # Re-download even when the cache already has this version.
    [switch]$Force
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$RepoRoot = (Get-Item $PSScriptRoot).Parent.Parent.FullName
$CacheDir = Join-Path $RepoRoot ".cache\nsis"
$NsisDir = Join-Path $CacheDir "nsis-$Version"
$MakeNsis = Join-Path $NsisDir "makensis.exe"
$Zip = Join-Path $CacheDir "nsis-$Version.zip"
$Url = "https://sourceforge.net/projects/nsis/files/NSIS%203/$Version/nsis-$Version.zip/download"

if ((Test-Path $MakeNsis) -and -not $Force) {
    Write-Host "NSIS $Version already cached: $MakeNsis"
    return
}

New-Item -ItemType Directory -Force $CacheDir | Out-Null

if ($Force -and (Test-Path $NsisDir)) {
    Remove-Item -Recurse -Force $NsisDir
}

if (-not (Test-Path $Zip) -or $Force) {
    Write-Host "Downloading NSIS $Version from $Url"
    # SourceForge answers /download with a redirect to a mirror, but only for
    # non-browser user agents; with the default PowerShell UA it serves a
    # ~140 KB "your download will start shortly" HTML page instead.
    Invoke-WebRequest -Uri $Url -OutFile $Zip -MaximumRedirection 10 -UseBasicParsing `
        -UserAgent "astrofin-fetch-nsis/1.0"
}

$actual = (Get-FileHash -Algorithm SHA256 -Path $Zip).Hash.ToLowerInvariant()
if ($Sha256 -and $actual -ne $Sha256.ToLowerInvariant()) {
    Remove-Item -Force $Zip
    throw "SHA-256 mismatch for nsis-$Version.zip: expected $Sha256, got $actual (file deleted)"
}
Write-Host "SHA-256 ok: $actual"

# The archive already has a nsis-<version>/ root directory.
Expand-Archive -Path $Zip -DestinationPath $CacheDir -Force

if (-not (Test-Path $MakeNsis)) {
    throw "unpacked NSIS but $MakeNsis is missing"
}

& $MakeNsis /VERSION | ForEach-Object { Write-Host "makensis $_ -> $MakeNsis" }
