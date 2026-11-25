# Build a portable, self-contained Windows package for tv-rust + spider-host.
#
# Layout produced:
#   dist\TVRust-win-x64\
#     TVRust.exe                 (release build of tv-rust)
#     mpv-2.dll                  (libmpv, required by the built-in player)
#     spider-host-all.jar        (JVM spider host, next to the exe)
#     runtime\bin\java.exe       (jlink-trimmed JRE, no system Java required)
#     LICENSE.txt
#     version.txt
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File packaging\package-windows.ps1
#   powershell -ExecutionPolicy Bypass -File packaging\package-windows.ps1 -GradleCmd <path-to-gradle.bat>
#   powershell -ExecutionPolicy Bypass -File packaging\package-windows.ps1 -SkipCargoBuild -SkipHostJar
#   powershell -ExecutionPolicy Bypass -File packaging\package-windows.ps1 -LibMpv D:\mpv\mpv-2.dll
#
# Notes:
#   - Gradle wrapper needs the distribution it pins; if that download is blocked,
#     pass -GradleCmd pointing at a local gradle.bat.
#   - The jlink runtime is reused when already present. Use -RecreateRuntime to rebuild it.
#   - libmpv (mpv-2.dll) is bundled next to the exe so the built-in player works
#     out of the box. The script looks for tv-rust\libmpv\dev\libmpv-2.dll and a few
#     other locations; use -LibMpv <path> to point at your own copy, or -SkipLibMpv
#     to build a package without the built-in player.

[CmdletBinding()]
param(
    [string]$Output = "",
    [string]$Jdk = "",
    [string]$GradleCmd = "",
    [string]$Version = "",
    [switch]$SkipHostJar,
    [switch]$SkipCargoBuild,
    [switch]$RecreateRuntime,
    [switch]$SkipLibMpv,
    [switch]$BundleLibMpv,
    [string]$LibMpv = "",
    [string]$LibMpvArchive = "",
    [string]$LibMpvUrl = "",
    [string]$LibMpvTag = "",
    [switch]$SkipZip
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
if (-not $Root) { $Root = (Get-Location).Path }
if (-not $Root) { throw "Cannot determine repo root (PSScriptRoot='$PSScriptRoot')" }

# jlink module list lives in packaging/common/jlink-modules.txt (shared by all platforms).
# Keep this file ASCII-only: PowerShell 5.1 parses UTF-8 without BOM as GBK, and a non-ASCII
# comment line can swallow the following code line (this actually bit us twice).
$ModulesFile = $Root + "\packaging\common\jlink-modules.txt"
if (-not (Test-Path -LiteralPath $ModulesFile)) { throw "Missing module list: $ModulesFile" }
$Modules = ((Get-Content -LiteralPath $ModulesFile) |
    ForEach-Object { $_.Trim() } |
    Where-Object { $_ -and -not $_.StartsWith("#") }) -join ","
if (-not $Modules) { throw "Empty module list: $ModulesFile" }

function Resolve-Tool([string]$explicit, [string]$name) {
    if ($explicit) {
        if (Test-Path $explicit -PathType Leaf) { return $explicit }
        $candidate = Join-Path $explicit ("bin\" + $name)
        if (Test-Path $candidate -PathType Leaf) { return (Resolve-Path $candidate).Path }
        throw "Cannot find $name at: $explicit"
    }
    if ($env:JAVA_HOME) {
        $candidate = Join-Path $env:JAVA_HOME ("bin\" + $name)
        if (Test-Path $candidate -PathType Leaf) { return (Resolve-Path $candidate).Path }
    }
    $cmd = Get-Command ($name -replace "\.exe$", "") -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    throw "$name not found. Pass -Jdk <jdk-dir> or set JAVA_HOME."
}

$jlink = Resolve-Tool $Jdk "jlink.exe"
$jdkHome = Split-Path -Parent (Split-Path -Parent $jlink)

# --- libmpv helpers -------------------------------------------------------
# libmpv (mpv-2.dll, ~118 MB) is NOT kept in the repo: GitHub rejects files
# over 100 MB. Instead it is downloaded once and cached under
# tv-rust\libmpv\cache\("cache" is git-ignored), so repeat builds are offline.

function Find-7Zip {
    # $env:ProgramFiles may be empty (stripped env / service session); collect roots first
    $roots = @()
    foreach ($v in @($env:ProgramW6432, $env:ProgramFiles, ${env:ProgramFiles(x86)}, "C:\Program Files", "D:\Program Files")) {
        if ($v) { $roots += $v }
    }
    foreach ($r in $roots) {
        $c = Join-Path $r "7-Zip\7z.exe"
        if (Test-Path -LiteralPath $c -PathType Leaf) { return $c }
    }
    # Registry fallback (7-Zip records its install dir)
    foreach ($key in @("HKLM:\SOFTWARE\7-Zip", "HKLM:\SOFTWARE\WOW6432Node\7-Zip")) {
        try {
            $base = (Get-ItemProperty -Path $key -ErrorAction Stop).Path
            if ($base) {
                $c = Join-Path $base "7z.exe"
                if (Test-Path -LiteralPath $c -PathType Leaf) { return $c }
            }
        } catch { }
    }
    $cmd = Get-Command "7z.exe" -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    return ""
}

function Expand-LibMpvFromArchive([string]$archive, [string]$destDir) {
    if (-not (Test-Path -LiteralPath $archive -PathType Leaf)) {
        throw "libmpv archive not found: $archive"
    }
    $sevenZip = Find-7Zip
    if (-not $sevenZip) {
        throw @"
Cannot unpack '$archive': 7-Zip is required (Windows tar.exe cannot read LZMA/7z).
Install 7-Zip, or pass -LibMpv <path-to-libmpv-2.dll> with an already extracted DLL.
"@
    }
    New-Item -ItemType Directory -Path $destDir -Force | Out-Null
    Write-Host "libmpv   : extracting libmpv-2.dll from $(Split-Path -Leaf $archive) ..."
    & $sevenZip x $archive "libmpv-2.dll" "-o$destDir" -y | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "7-Zip extraction failed ($LASTEXITCODE): $archive" }
    $dll = Join-Path $destDir "libmpv-2.dll"
    if (-not (Test-Path -LiteralPath $dll -PathType Leaf)) {
        throw "Archive does not contain libmpv-2.dll: $archive"
    }
    return $dll
}

function Save-LibMpvArchive([string]$url, [string]$destFile) {
    Write-Host "libmpv   : downloading $url"
    $dir = Split-Path -Parent $destFile
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    $tmp = "$destFile.part"
    if (Test-Path -LiteralPath $tmp) { Remove-Item -LiteralPath $tmp -Force }
    try {
        Invoke-WebRequest -Uri $url -OutFile $tmp -UseBasicParsing
    } catch {
        throw @"
libmpv download failed: $url
$($_.Exception.Message)
If the network is restricted, download the mpv-dev archive manually and pass
  -LibMpvArchive <path-to-mpv-dev-x86_64-*.7z>
"@
    }
    Move-Item -LiteralPath $tmp -Destination $destFile -Force
    return $destFile
}

function Resolve-LibMpvArchiveUrl {
    # Pinned tag wins; otherwise ask the GitHub API for the newest mpv-dev x86_64 asset.
    $api = if ($LibMpvTag) {
        "https://api.github.com/repos/zhongfly/mpv-winbuild/releases/tags/$LibMpvTag"
    } else {
        "https://api.github.com/repos/zhongfly/mpv-winbuild/releases/latest"
    }
    Write-Host "libmpv   : resolving release via $api"
    $rel = Invoke-RestMethod -Uri $api -Headers @{ "User-Agent" = "TVRust-packaging" }
    $asset = $rel.assets |
        Where-Object { $_.name -like "mpv-dev-x86_64-*.7z" } |
        Select-Object -First 1
    if (-not $asset) { throw "No mpv-dev-x86_64-*.7z asset in release '$($rel.tag_name)'" }
    Write-Host ("libmpv   : release {0}, asset {1} ({2:N1} MB)" -f $rel.tag_name, $asset.name, ($asset.size / 1MB))
    return $asset.browser_download_url
}

if (-not $Version) {
    $cargoToml = Join-Path $Root "tv-rust\Cargo.toml"
    if (Test-Path $cargoToml) {
        $m = Select-String -Path $cargoToml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
        if ($m) { $Version = $m.Matches[0].Groups[1].Value }
    }
    if (-not $Version) { $Version = "0.0.0" }
}

if (-not $Output) {
    $Output = Join-Path $Root "dist\TVRust-win-x64"
}
$Output = [System.IO.Path]::GetFullPath($Output)

Write-Host "== tv-rust portable package =="
Write-Host "repo   : $Root"
Write-Host "output : $Output"
Write-Host "version: $Version"
Write-Host "jdk    : $jdkHome"

# 1) spider-host fat jar
$jar = Join-Path $Root "spider-host\build\libs\spider-host-all.jar"
if (-not $SkipHostJar) {
    # spider-host is a standalone Gradle project: wrapper and build live under spider-host/.
    # The repo root has no Gradle build any more; task paths like `:spider-host:fatJar` no longer exist.
    $hostDir = Join-Path $Root "spider-host"
    $gradle = $GradleCmd
    if (-not $gradle) { $gradle = Join-Path $hostDir "gradlew.bat" }
    if (-not (Test-Path $gradle)) { throw "Gradle not found: $gradle" }
    Write-Host "building spider-host fatJar ..."
    Push-Location $hostDir
    try {
        & $gradle --console=plain fatJar
        if ($LASTEXITCODE -ne 0) { throw "gradle fatJar failed ($LASTEXITCODE)" }
    } finally {
        Pop-Location
    }
}
if (-not (Test-Path $jar)) { throw "Missing host jar: $jar" }

# 2) release binary
$tvRustDir = Join-Path $Root "tv-rust"
$exeName = "tv_rust.exe"
$exePath = Join-Path $tvRustDir ("target\release\" + $exeName)
if (-not $SkipCargoBuild) {
    Write-Host "building : cargo build --release ..."
    Push-Location $tvRustDir
    try {
        cargo build --release
        if ($LASTEXITCODE -ne 0) { throw "cargo build --release failed ($LASTEXITCODE)" }
    } finally {
        Pop-Location
    }
}
if (-not (Test-Path $exePath)) { throw "Missing release binary: $exePath" }

# 2b) libmpv: the built-in player downloads it on demand from the app.
#     Default = DO NOT bundle (the package stays ~95 MB instead of ~142 MB).
#     Opt in with -BundleLibMpv, or by passing -LibMpv / -LibMpvArchive / -LibMpvUrl.
#     Resolution order: -LibMpv dll -> repo candidates -> cache -> archive -> download.
$bundleLibMpv = $BundleLibMpv -or ($LibMpv -ne "") -or ($LibMpvArchive -ne "") -or ($LibMpvUrl -ne "")
if ($SkipLibMpv) { $bundleLibMpv = $false }
$libMpvSrc = ""
if ($bundleLibMpv) {
    $libMpvCacheDir = Join-Path $Root "tv-rust\libmpv\cache"
    $cachedDll = Join-Path $libMpvCacheDir "libmpv-2.dll"

    if ($LibMpv) {
        if (-not (Test-Path -LiteralPath $LibMpv -PathType Leaf)) {
            throw "libmpv not found at -LibMpv path: $LibMpv"
        }
        $libMpvSrc = (Resolve-Path -LiteralPath $LibMpv).Path
    } else {
        $candidates = @(
            $cachedDll,
            (Join-Path $Root "tv-rust\libmpv\dev\libmpv-2.dll"),
            (Join-Path $Root "tv-rust\libmpv\libmpv-2.dll"),
            (Join-Path $Root "tv-rust\libmpv\mpv-2.dll"),
            (Join-Path $Root "dist\mpv\mpv-2.dll")
        )
        $libMpvSrc = $candidates |
            Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } |
            Select-Object -First 1
    }

    if (-not $libMpvSrc) {
        # A local archive (offline path) first, then a URL, then the GitHub API.
        $archive = $LibMpvArchive
        if (-not $archive) {
            foreach ($c in @(
                (Join-Path $Root "tv-rust\libmpv\mpv-dev.7z"),
                (Join-Path $Root "tv-rust\libmpv\cache\mpv-dev.7z"))) {
                if (Test-Path -LiteralPath $c -PathType Leaf) { $archive = $c; break }
            }
        }
        if (-not $archive) {
            $url = $LibMpvUrl
            if (-not $url) { $url = Resolve-LibMpvArchiveUrl }
            $archive = Save-LibMpvArchive $url (Join-Path $libMpvCacheDir "mpv-dev.7z")
        }
        $libMpvSrc = Expand-LibMpvFromArchive $archive $libMpvCacheDir
    }

    $libMpvSrc = (Resolve-Path -LiteralPath $libMpvSrc).Path
    Write-Host "libmpv   : $libMpvSrc"
} else {
    Write-Host "libmpv   : not bundled (app downloads it on demand; use -BundleLibMpv to bundle)"
}

# 3) assemble
New-Item -ItemType Directory -Path $Output -Force | Out-Null
$runtimeDir = Join-Path $Output "runtime"
if ($RecreateRuntime -and (Test-Path $runtimeDir)) {
    Remove-Item $runtimeDir -Recurse -Force
}
if (-not (Test-Path $runtimeDir)) {
    Write-Host "jlink    : $Modules"
    & $jlink --add-modules $Modules --strip-debug --no-header-files --no-man-pages --compress=zip-6 --output $runtimeDir
    if ($LASTEXITCODE -ne 0) { throw "jlink failed ($LASTEXITCODE)" }
} else {
    Write-Host "jlink    : reuse existing runtime"
}
if (-not (Test-Path (Join-Path $runtimeDir "bin\java.exe"))) { throw "jlink runtime is incomplete: $runtimeDir" }

Copy-Item $exePath (Join-Path $Output "TVRust.exe") -Force
Copy-Item $jar (Join-Path $Output "spider-host-all.jar") -Force

# libmpv sits next to the exe; settings.rs looks for mpv-2.dll there first.
if ($bundleLibMpv) {
    $mpvDst = Join-Path $Output "mpv-2.dll"
    Copy-Item $libMpvSrc $mpvDst -Force
    $mpvMb = [math]::Round((Get-Item $mpvDst).Length / 1MB, 1)
    Write-Host ("libmpv   : copied mpv-2.dll ({0} MB)" -f $mpvMb)
}

$license = Join-Path $Root "LICENSE"
if (Test-Path $license) { Copy-Item $license (Join-Path $Output "LICENSE.txt") -Force }

$stamp = Get-Date -Format "yyyy-MM-dd HH:mm:ss"
@(
    "TVRust portable package",
    "version : $Version",
    "built   : $stamp",
    "runtime : jlink $Modules",
    $(if ($bundleLibMpv) { "libmpv  : mpv-2.dll bundled (built-in player)" } else { "libmpv  : not bundled (app downloads it in Settings > Player)" }),
    "",
    "Run TVRust.exe. The bundled runtime under runtime\bin is used automatically;",
    "no system-wide Java installation is required.",
    "Data (settings/history/config cache) is stored in %AppData%\TV-Multiplatform."
) | Set-Content -Path (Join-Path $Output "version.txt") -Encoding UTF8

# 4) zip
$zip = Join-Path (Split-Path -Parent $Output) ("TVRust-{0}-win-x64.zip" -f $Version)
if ($SkipZip) {
    Write-Host "zip      : skipped (-SkipZip)"
} else {
    if (Test-Path $zip) { Remove-Item $zip -Force }
    Write-Host "zip      : $zip"
    Compress-Archive -Path (Join-Path $Output "*") -DestinationPath $zip -CompressionLevel Optimal
}

$exeMb = [math]::Round((Get-Item (Join-Path $Output "TVRust.exe")).Length / 1MB, 1)
$jarMb = [math]::Round((Get-Item (Join-Path $Output "spider-host-all.jar")).Length / 1MB, 1)
$rtMb = [math]::Round(((Get-ChildItem $runtimeDir -Recurse -File | Measure-Object Length -Sum).Sum / 1MB), 1)
Write-Host ""
Write-Host "done:"
Write-Host ("  exe     : {0} MB" -f $exeMb)
Write-Host ("  host jar: {0} MB" -f $jarMb)
Write-Host ("  runtime : {0} MB" -f $rtMb)
if ($bundleLibMpv) {
    $mpvFinal = Join-Path $Output "mpv-2.dll"
    if (-not (Test-Path -LiteralPath $mpvFinal)) { throw "libmpv missing from package: $mpvFinal" }
    $mpvMb = [math]::Round((Get-Item $mpvFinal).Length / 1MB, 1)
    Write-Host ("  libmpv  : {0} MB  (mpv-2.dll)" -f $mpvMb)
}
if (-not $SkipZip) {
    $zipMb = [math]::Round((Get-Item $zip).Length / 1MB, 1)
    Write-Host ("  zip     : {0} MB  ({1})" -f $zipMb, $zip)
}
