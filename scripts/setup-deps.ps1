# Fetches vcpkg into .deps\vcpkg (pinned commit) and builds libheif with only
# the libde265 HEVC decoder, as static libraries with the static CRT, so that
# heic_wic.dll and heic-convert.exe are fully self-contained.
$ErrorActionPreference = 'Stop'

# vcpkg commit providing libheif 1.23.7 / libde265 1.1.3. Bump deliberately.
$VcpkgCommit = 'd13c536672dced4b8c671955ea7d057a1b7a1ff8'
$Triplet = 'x64-windows-static'

$root = Split-Path -Parent $PSScriptRoot
$vcpkg = Join-Path $root '.deps\vcpkg'

if (-not (Test-Path (Join-Path $vcpkg '.git'))) {
    New-Item -ItemType Directory -Force $vcpkg | Out-Null
    git -C $vcpkg init -q
    git -C $vcpkg remote add origin https://github.com/microsoft/vcpkg.git
}
$current = (git -C $vcpkg rev-parse HEAD 2>$null)
if ($current -ne $VcpkgCommit) {
    git -C $vcpkg fetch -q --depth 1 origin $VcpkgCommit
    git -C $vcpkg checkout -q --force FETCH_HEAD
    Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $vcpkg 'vcpkg.exe')
}
if (-not (Test-Path (Join-Path $vcpkg 'vcpkg.exe'))) {
    & (Join-Path $vcpkg 'bootstrap-vcpkg.bat') -disableMetrics
    if ($LASTEXITCODE -ne 0) { throw 'vcpkg bootstrap failed' }
}

# Decoder only: no x265 encoder (GPL, and not needed).
& (Join-Path $vcpkg 'vcpkg.exe') install "libheif[core,libde265]:$Triplet" --clean-after-build
if ($LASTEXITCODE -ne 0) { throw 'vcpkg install failed' }
Write-Host 'Dependencies ready. Build with: cargo build --release'
