# Build the CPU-only runtime with the same static CRT as the release executables.
# Requires PowerShell 7.3+, Visual Studio C++ tools, CMake, Ninja, Python and Git.
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true

$project = Split-Path $PSScriptRoot -Parent
$cache = Join-Path $project ".cache/$(Get-Date -Format yyyy-MM-dd)/onnxruntime-windows"
$source = Join-Path $cache "source"
$build = Join-Path $cache "build"
$revision = "c4fb724e810bb496165b9015c77f402727392933" # ONNX Runtime v1.20.0 commit, not its annotated tag object
New-Item -ItemType Directory -Force $cache | Out-Null

if (!(Test-Path (Join-Path $source ".git"))) {
    git init $source
    git -C $source remote add origin https://github.com/microsoft/onnxruntime.git
    git -C $source fetch --depth 1 origin $revision
    git -C $source checkout --detach FETCH_HEAD
}
if ((git -C $source rev-parse HEAD).Trim() -ne $revision) {
    throw "Unexpected ONNX Runtime source revision in $source"
}
git -C $source submodule update --init --recursive

# The GitLab archive no longer matches ORT's hash; fetch the same pinned source commit.
$eigen = Join-Path $cache "eigen"
$eigenRevision = "e7248b26a1ed53fa030c5c459f7ea095dfd276ac"
if (!(Test-Path (Join-Path $eigen ".git"))) {
    git init $eigen
    git -C $eigen remote add origin https://gitlab.com/libeigen/eigen.git
    git -C $eigen fetch --depth 1 origin $eigenRevision
    git -C $eigen checkout --detach FETCH_HEAD
}
if ((git -C $eigen rev-parse HEAD).Trim() -ne $eigenRevision) {
    throw "Unexpected Eigen source revision in $eigen"
}

$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$visualStudio = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)
if (!$visualStudio) { throw "Visual Studio x64 C++ tools are required" }
& (Join-Path $visualStudio "Common7/Tools/Launch-VsDevShell.ps1") -Arch amd64 -HostArch amd64 -SkipAutomaticLocation

# New MSVC releases warn about attributes used by this pinned third-party runtime.
# ORT 1.20's mutex header also relies on a transitive <chrono> include removed in newer MSVC.
# Its pinned Eigen still uses the standard denorm types deprecated by current MSVC.
cmake --compile-no-warning-as-error -S (Join-Path $source "cmake") -B $build -G Ninja `
    -DCMAKE_BUILD_TYPE=Release `
    "-DCMAKE_CXX_FLAGS=/FIchrono /D_SILENCE_CXX23_DENORM_DEPRECATION_WARNING" `
    -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded `
    -DONNX_USE_MSVC_STATIC_RUNTIME=ON `
    -Dprotobuf_MSVC_STATIC_RUNTIME=ON `
    -DABSL_MSVC_STATIC_RUNTIME=ON `
    -Dgtest_force_shared_crt=OFF `
    "-DCMAKE_POLICY_VERSION_MINIMUM=3.5" `
    -Donnxruntime_USE_PREINSTALLED_EIGEN=ON `
    "-Deigen_SOURCE_PATH=$eigen" `
    -Donnxruntime_BUILD_UNIT_TESTS=OFF `
    -Donnxruntime_BUILD_SHARED_LIB=OFF `
    -Donnxruntime_USE_DML=OFF `
    -Donnxruntime_USE_XNNPACK=OFF `
    -Donnxruntime_ENABLE_LTO=OFF
cmake --build $build --config Release --parallel 4

if (!(Test-Path (Join-Path $build "onnxruntime_session.lib"))) {
    throw "Static ONNX Runtime libraries were not produced"
}
# ort-sys understands this CMake output layout, including its _deps libraries.
$env:ORT_LIB_LOCATION = $build
if ($env:GITHUB_ENV) {
    "ORT_LIB_LOCATION=$build" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append
}
Write-Host "Static-CRT ONNX Runtime ready: $build"
