# Build a portable, self-contained Windows package for tv-rust + spider-host.
#
# Layout produced:
#   dist\TVRust-win-x64\
#     TVRust.exe                 (release build of tv-rust)
#     spider-host-all.jar        (JVM spider host, next to the exe)
#     runtime\bin\java.exe       (jlink-trimmed JRE, no system Java required)
#     LICENSE.txt
#     version.txt
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File packaging\package-windows.ps1
#   powershell -ExecutionPolicy Bypass -File packaging\package-windows.ps1 -GradleCmd <path-to-gradle.bat>
#   powershell -ExecutionPolicy Bypass -File packaging\package-windows.ps1 -SkipCargoBuild -SkipHostJar
#
# Notes:
#   - Gradle wrapper needs the distribution it pins; if that download is blocked,
#     pass -GradleCmd pointing at a local gradle.bat.
#   - The jlink runtime is reused when already present. Use -RecreateRuntime to rebuild it.

[CmdletBinding()]
param(
    [string]$Output = "",
    [string]$Jdk = "",
    [string]$GradleCmd = "",
    [string]$Version = "",
    [switch]$SkipHostJar,
    [switch]$SkipCargoBuild,
    [switch]$RecreateRuntime
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
    $gradle = $GradleCmd
    if (-not $gradle) { $gradle = Join-Path $Root "gradlew.bat" }
    if (-not (Test-Path $gradle)) { throw "Gradle not found: $gradle" }
    Write-Host "building :spider-host:fatJar ..."
    Push-Location $Root
    try {
        & $gradle --console=plain :spider-host:fatJar
        if ($LASTEXITCODE -ne 0) { throw "gradle :spider-host:fatJar failed ($LASTEXITCODE)" }
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

$license = Join-Path $Root "LICENSE"
if (Test-Path $license) { Copy-Item $license (Join-Path $Output "LICENSE.txt") -Force }

$stamp = Get-Date -Format "yyyy-MM-dd HH:mm:ss"
@(
    "TVRust portable package",
    "version : $Version",
    "built   : $stamp",
    "runtime : jlink $Modules",
    "",
    "Run TVRust.exe. The bundled runtime under runtime\bin is used automatically;",
    "no system-wide Java installation is required.",
    "Data (settings/history/config cache) is stored in %AppData%\TV-Multiplatform."
) | Set-Content -Path (Join-Path $Output "version.txt") -Encoding UTF8

# 4) zip
$zip = Join-Path (Split-Path -Parent $Output) ("TVRust-{0}-win-x64.zip" -f $Version)
if (Test-Path $zip) { Remove-Item $zip -Force }
Write-Host "zip      : $zip"
Compress-Archive -Path (Join-Path $Output "*") -DestinationPath $zip -CompressionLevel Optimal

$exeMb = [math]::Round((Get-Item (Join-Path $Output "TVRust.exe")).Length / 1MB, 1)
$jarMb = [math]::Round((Get-Item (Join-Path $Output "spider-host-all.jar")).Length / 1MB, 1)
$rtMb = [math]::Round(((Get-ChildItem $runtimeDir -Recurse -File | Measure-Object Length -Sum).Sum / 1MB), 1)
$zipMb = [math]::Round((Get-Item $zip).Length / 1MB, 1)
Write-Host ""
Write-Host "done:"
Write-Host ("  exe     : {0} MB" -f $exeMb)
Write-Host ("  host jar: {0} MB" -f $jarMb)
Write-Host ("  runtime : {0} MB" -f $rtMb)
Write-Host ("  zip     : {0} MB  ({1})" -f $zipMb, $zip)
