# 构建脚本：使用便携版 mingw-w64 (w64devkit) + Rust windows-gnu 目标
# 无需安装 Visual Studio，无需管理员权限
#
# 用法:  powershell -ExecutionPolicy Bypass -File scripts/build.ps1 [-Release]

param(
    [switch]$Release
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$tools = Join-Path $env:LOCALAPPDATA "FileSync-tools"
$mingw = Join-Path $tools "w64devkit"

if (-not (Test-Path (Join-Path $mingw "bin\gcc.exe"))) {
    Write-Host "未找到 w64devkit，请先运行 scripts/setup-toolchain.ps1" -ForegroundColor Red
    exit 1
}

# 把 mingw 加入 PATH，rustc 通过 gcc 链接
$env:PATH = (Join-Path $mingw "bin") + ";" + $env:PATH
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = Join-Path $mingw "bin\gcc.exe"
$env:CC = Join-Path $mingw "bin\gcc.exe"
$env:AR = Join-Path $mingw "bin\ar.exe"
$env:WINDRES = Join-Path $mingw "bin\windres.exe"

$target = "x86_64-pc-windows-gnu"
$manifest = Join-Path $root "src-tauri\Cargo.toml"

# 关键：必须使用 gnu 宿主的 rustup 工具链。
# 默认的 stable-x86_64-pc-windows-msvc 在编译 build script / proc-macro（宿主产物）时
# 会调用 link.exe，而本机没有 MSVC 工具链。换成 gnu 宿主后宿主产物同样用 gcc 链接。
$toolchain = "stable-x86_64-pc-windows-gnu"
$tcVer = & rustup run $toolchain rustc --version 2>&1
if ($LASTEXITCODE -ne 0) {
    Write-Host "缺少 rustup 工具链 $toolchain，请先运行 scripts/setup-toolchain.ps1" -ForegroundColor Red
    exit 1
}

$cargoArgs = @("+$toolchain", "build", "--manifest-path", $manifest, "--target", $target)
if ($Release) { $cargoArgs += "--release" }

Write-Host "cargo $($cargoArgs -join ' ')" -ForegroundColor Cyan
& cargo @cargoArgs
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$out = if ($Release) { "release" } else { "debug" }
$dir = Join-Path $root "src-tauri\target\$target\$out"
# 二进制名取自 Cargo 包名（filesync.exe），此处按实际产物查找，避免大小写差异
$exe = Get-ChildItem $dir -File -Filter "*.exe" -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -notmatch "^(uninstall|File \w+-copy)" } |
    Select-Object -First 1
Write-Host ""
if ($exe) {
    $size = [math]::Round($exe.Length / 1MB, 2)
    Write-Host "构建成功: $($exe.FullName)  ($size MB)" -ForegroundColor Green
    Write-Host "注意: 该 exe 必须与 WebView2Loader.dll 同目录才能运行（可用 scripts/build-installer.ps1 打安装包）" -ForegroundColor DarkGray
} else {
    Write-Host "构建完成，但未找到产物: $dir" -ForegroundColor Yellow
}

exit 0
