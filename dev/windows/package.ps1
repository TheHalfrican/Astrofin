# Build the Windows installers for Astrofin from the already-staged build\ tree.
#
#   pwsh -ExecutionPolicy Bypass -File dev\windows\package.ps1
#
# This does NOT build the app; run dev\windows\build.ps1 first. It only takes
# what build.ps1 staged in build\ (astrofin.exe + the CEF runtime + the mpv
# DLLs) and wraps it in:
#
#   dist\Astrofin-<version>-x64-setup.exe   NSIS, per-user, no elevation
#   dist\Astrofin-<version>-x64.msi         WiX 5, per-user by default
#
# Tooling (both user-scope; nothing needs admin):
#   NSIS  - .cache\nsis\nsis-<ver>\makensis.exe, fetched by dev\windows\fetch-nsis.ps1
#   WiX 5 - `dotnet tool install --global wix --version 5.*`
#           plus `wix extension add --global WixToolset.UI.wixext/5.0.2`

[CmdletBinding()]
param(
    # Which installers to produce.
    [ValidateSet("both", "nsis", "msi")]
    [string]$Only = "both",

    # NSIS compressor. lzma is much smaller but takes minutes over a ~600 MB
    # payload; zlib is the fast option for local iteration.
    [ValidateSet("lzma", "bzip2", "zlib")]
    [string]$Compressor = "lzma",

    # WiX cabinet compression: high (smallest) / medium / low / mszip / none.
    [ValidateSet("high", "medium", "low", "mszip", "none")]
    [string]$MsiCompression = "high",

    # Reuse the payload directory from a previous run instead of re-mirroring
    # build\ into it.
    [switch]$SkipPayloadRefresh,

    # ---- Code-signing hook -------------------------------------------------
    # No signing certificate exists for this project yet. Point -SignTool at a
    # signtool.exe (or set ASTROFIN_SIGNTOOL) and pass the certificate/timestamp
    # arguments in -SignArgs / ASTROFIN_SIGNTOOL_ARGS to sign astrofin.exe
    # before packaging and both installers afterwards. When unset, signing is
    # skipped and the script says so.
    [string]$SignTool = $env:ASTROFIN_SIGNTOOL,
    [string[]]$SignArgs = @()
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$RepoRoot    = (Get-Item $PSScriptRoot).Parent.Parent.FullName
$BuildDir    = Join-Path $RepoRoot "build"
$DistDir     = Join-Path $RepoRoot "dist"
$PayloadDir  = Join-Path $BuildDir "installer-payload"
$WorkDir     = Join-Path $BuildDir "installer-work"
$InstallerSrc= Join-Path $RepoRoot "dev\windows\installer"
$IconFile    = Join-Path $RepoRoot "resources\win\astrofin.ico"
$LicenseSrc  = Join-Path $RepoRoot "LICENSE"
$AppExe      = Join-Path $BuildDir "astrofin.exe"

if (-not $SignArgs -and $env:ASTROFIN_SIGNTOOL_ARGS) {
    $SignArgs = $env:ASTROFIN_SIGNTOOL_ARGS -split '\s+'
}

function Fail([string]$msg) {
    Write-Host "package.ps1: $msg" -ForegroundColor Red
    exit 1
}

#---------------------------------------------------------------------------
# 1. The staged tree must exist
#---------------------------------------------------------------------------
if (-not (Test-Path $AppExe)) {
    Fail @"
$AppExe not found.

Build and stage the app first:
    pwsh -ExecutionPolicy Bypass -File dev\windows\build.ps1
"@
}
if (-not (Test-Path $IconFile)) { Fail "$IconFile not found" }

#---------------------------------------------------------------------------
# 2. Version
#---------------------------------------------------------------------------
# The display version is read from the staged binary's VERSIONINFO
# (ProductVersion, written by src/jfn_rust/build.rs from resources/win/iconres.rc.in),
# so the installer always names exactly the binary it contains — including the
# git suffix, e.g. "0.1.0-dev+3208f44-dirty". The Cargo manifest version is only
# a fallback for a binary without version resources.
#
# Windows Installer ProductVersion must be numeric "a.b.c" with a,b <= 255 and
# c <= 65535, so the MSI version is the leading a.b.c of the display version with
# any pre-release/build metadata ("-dev", "+<sha>", "-dirty") stripped. The full
# string is kept in the file name and in ARP DisplayVersion via SummaryInformation.
$DisplayVersion = (Get-Item $AppExe).VersionInfo.ProductVersion
if (-not $DisplayVersion) {
    $cargo = Get-Content (Join-Path $RepoRoot "src\Cargo.toml")
    $DisplayVersion = ($cargo | Select-String -Pattern '^\s*version\s*=\s*"([^"]+)"' |
        Select-Object -First 1).Matches[0].Groups[1].Value
    Write-Host "VERSIONINFO missing; using the Cargo manifest version instead"
}
$DisplayVersion = $DisplayVersion.Trim()

if ($DisplayVersion -notmatch '^(\d+)\.(\d+)\.(\d+)') {
    Fail "cannot derive an MSI ProductVersion from '$DisplayVersion' (need a leading a.b.c)"
}
$major = [int]$Matches[1]; $minor = [int]$Matches[2]; $patch = [int]$Matches[3]
if ($major -gt 255 -or $minor -gt 255 -or $patch -gt 65535) {
    Fail "version $major.$minor.$patch is out of range for an MSI ProductVersion (a,b <= 255, c <= 65535)"
}
$ProductVersion = "$major.$minor.$patch"
$FileVersion4   = "$major.$minor.$patch.0"

Write-Host "Astrofin installers"
Write-Host "  display version : $DisplayVersion"
Write-Host "  MSI/file version: $ProductVersion"
Write-Host "  payload source  : $BuildDir"

#---------------------------------------------------------------------------
# 3. Mirror the staged tree into a clean payload directory
#---------------------------------------------------------------------------
# build\ also holds the cargo target dir, the `cargo xtask package` prefix and
# run logs; none of that belongs in an installer, and there is no explicit file
# list because the CEF runtime is ~130 DLLs plus locales\ and .pak/.dat data.
New-Item -ItemType Directory -Force $DistDir | Out-Null
New-Item -ItemType Directory -Force $WorkDir | Out-Null

if (-not $SkipPayloadRefresh) {
    New-Item -ItemType Directory -Force $PayloadDir | Out-Null
    $excludeDirs = @(
        (Join-Path $BuildDir "cargo-target"),
        (Join-Path $BuildDir "install"),
        (Join-Path $BuildDir "mpv-build"),
        $PayloadDir,
        $WorkDir
    )
    Write-Host "Mirroring $BuildDir -> $PayloadDir"
    $rc = @($BuildDir, $PayloadDir, "/MIR", "/NFL", "/NDL", "/NJH", "/NJS", "/NP", "/R:1", "/W:1",
            "/XD") + $excludeDirs + @("/XF", "*.log")
    & robocopy.exe @rc | Out-Null
    # robocopy: 0-7 are success codes, >= 8 is a real failure.
    if ($LASTEXITCODE -ge 8) { Fail "robocopy failed with exit code $LASTEXITCODE" }
}
if (-not (Test-Path (Join-Path $PayloadDir "astrofin.exe"))) {
    Fail "$PayloadDir\astrofin.exe missing after staging"
}

$payloadFiles = Get-ChildItem $PayloadDir -Recurse -File
$payloadBytes = ($payloadFiles | Measure-Object -Property Length -Sum).Sum
Write-Host ("  payload         : {0} files, {1:N1} MB" -f $payloadFiles.Count, ($payloadBytes / 1MB))

#---------------------------------------------------------------------------
# 4. Optional signing hook (payload binaries)
#---------------------------------------------------------------------------
function Invoke-Sign([string[]]$Files) {
    if (-not $SignTool) { return }
    if (-not (Test-Path $SignTool)) { Fail "-SignTool '$SignTool' not found" }
    foreach ($f in $Files) {
        Write-Host "Signing $f"
        & $SignTool sign @SignArgs $f
        if ($LASTEXITCODE -ne 0) { Fail "signtool failed for $f (exit $LASTEXITCODE)" }
    }
}
if ($SignTool) {
    Invoke-Sign @((Join-Path $PayloadDir "astrofin.exe"))
} else {
    Write-Host "  signing         : skipped (no -SignTool / ASTROFIN_SIGNTOOL)"
}

$outputs = @()

#---------------------------------------------------------------------------
# 5. NSIS
#---------------------------------------------------------------------------
if ($Only -in @("both", "nsis")) {
    # Throws (and aborts this script) if the download or hash check fails.
    & (Join-Path $PSScriptRoot "fetch-nsis.ps1")

    $makensis = Get-ChildItem (Join-Path $RepoRoot ".cache\nsis") -Filter "makensis.exe" -Recurse -Depth 1 |
        Sort-Object FullName -Descending | Select-Object -First 1
    if (-not $makensis) { Fail "makensis.exe not found under .cache\nsis" }

    $setupExe = Join-Path $DistDir "Astrofin-$DisplayVersion-x64-setup.exe"
    if (Test-Path $setupExe) { Remove-Item -Force $setupExe }

    Write-Host "Building $([IO.Path]::GetFileName($setupExe)) (compressor: $Compressor)"
    $sw = [Diagnostics.Stopwatch]::StartNew()
    & $makensis.FullName `
        "/V2" `
        "/DAPP_VERSION=$DisplayVersion" `
        "/DAPP_VERSION_NUM=$FileVersion4" `
        "/DPAYLOAD_DIR=$PayloadDir" `
        "/DICON_FILE=$IconFile" `
        "/DOUT_FILE=$setupExe" `
        "/DCOMPRESSOR=$Compressor" `
        (Join-Path $InstallerSrc "astrofin.nsi")
    if ($LASTEXITCODE -ne 0) { Fail "makensis failed with exit code $LASTEXITCODE" }
    $sw.Stop()
    Write-Host ("  makensis took {0:N0}s" -f $sw.Elapsed.TotalSeconds)
    Invoke-Sign @($setupExe)
    $outputs += $setupExe
}

#---------------------------------------------------------------------------
# 6. WiX MSI
#---------------------------------------------------------------------------
if ($Only -in @("both", "msi")) {
    $wix = Get-Command wix -ErrorAction SilentlyContinue
    if (-not $wix) {
        $candidate = Join-Path $env:USERPROFILE ".dotnet\tools\wix.exe"
        if (Test-Path $candidate) {
            $wix = Get-Item $candidate
        } else {
            Fail @"
`wix` not found. Install the WiX 5 toolset as a user-scope dotnet tool:
    dotnet tool install --global wix --version 5.*
    wix extension add --global WixToolset.UI.wixext/5.0.2
"@
        }
    }

    # WixUI_InstallDir shows a licence page; render LICENSE (GPL-2.0) into RTF
    # so the MSI does not ship WiX's placeholder text.
    $licenseRtf = Join-Path $WorkDir "license.rtf"
    $body = (Get-Content -Raw $LicenseSrc) -replace '\\', '\\\\' -replace '\{', '\{' -replace '\}', '\}'
    $body = ($body -split "\r?\n") -join "\par`r`n"
    Set-Content -LiteralPath $licenseRtf -Encoding ascii -Value `
        ("{\rtf1\ansi\deff0{\fonttbl{\f0\fnil\fcharset0 Segoe UI;}}`r`n\fs18`r`n" + $body + "`r`n}")

    $msi = Join-Path $DistDir "Astrofin-$DisplayVersion-x64.msi"
    if (Test-Path $msi) { Remove-Item -Force $msi }

    Write-Host "Building $([IO.Path]::GetFileName($msi)) (cab compression: $MsiCompression)"
    $sw = [Diagnostics.Stopwatch]::StartNew()
    # Note: `wix build` does not run ICE validation (that is a separate
    # `wix msi validate` step). The per-user/per-machine ICEs — ICE38/ICE64/ICE91
    # for components under LocalAppDataFolder, ICE43/ICE57 for the HKCU-keypath
    # shortcut components — would fire on this package by design, so validation
    # is deliberately not wired in here.
    & $wix.Source build `
        -arch x64 `
        -ext WixToolset.UI.wixext `
        -d "PayloadDir=$PayloadDir" `
        -d "IconFile=$IconFile" `
        -d "LicenseRtf=$licenseRtf" `
        -d "ProductVersion=$ProductVersion" `
        -d "DisplayVersion=$DisplayVersion" `
        -d "CompressionLevel=$MsiCompression" `
        -cabcache (Join-Path $WorkDir "cabcache") `
        -intermediatefolder (Join-Path $WorkDir "wix-obj") `
        -o $msi `
        (Join-Path $InstallerSrc "astrofin.wxs")
    if ($LASTEXITCODE -ne 0) { Fail "wix build failed with exit code $LASTEXITCODE" }
    $sw.Stop()
    Write-Host ("  wix build took {0:N0}s" -f $sw.Elapsed.TotalSeconds)
    Invoke-Sign @($msi)
    $outputs += $msi
}

#---------------------------------------------------------------------------
# 7. Report
#---------------------------------------------------------------------------
Write-Host ""
Write-Host "Installers:" -ForegroundColor Green
foreach ($o in $outputs) {
    if (-not (Test-Path $o)) { Fail "expected output $o was not produced" }
    $item = Get-Item $o
    $hash = (Get-FileHash -Algorithm SHA256 $o).Hash.ToLowerInvariant()
    Write-Host ("  {0}" -f $item.FullName)
    Write-Host ("      {0:N1} MB  sha256 {1}" -f ($item.Length / 1MB), $hash)
}
if ($outputs.Count -eq 0) { Fail "nothing was built" }
exit 0
