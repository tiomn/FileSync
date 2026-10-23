# 一次性准备构建工具链（无需管理员权限）
#   - 下载便携版 mingw-w64 (w64devkit) 到 %LOCALAPPDATA%\FileSync-tools
#   - 安装 Rust 的 x86_64-pc-windows-gnu 标准库
#
# 用法:  powershell -ExecutionPolicy Bypass -File scripts/setup-toolchain.ps1

$ErrorActionPreference = "Stop"

$tools = Join-Path $env:LOCALAPPDATA "FileSync-tools"
$mingw = Join-Path $tools "w64devkit"
$version = "2.10.1"
$gh = "https://github.com/skeeto/w64devkit/releases/download/v$version/w64devkit-x64-$version.7z.exe"
# GitHub 直连不可达时的备用代理，按顺序尝试
$urls = @($gh, "https://ghfast.top/$gh", "https://ghproxy.net/$gh", "https://gh-proxy.com/$gh")

New-Item -ItemType Directory -Force -Path $tools | Out-Null

if (Test-Path (Join-Path $mingw "bin\gcc.exe")) {
    Write-Host "w64devkit 已存在，跳过下载" -ForegroundColor Green
} else {
    $exe = Join-Path $tools "w64devkit.7z.exe"
    $ok = $false
    foreach ($u in $urls) {
        try {
            Write-Host "正在下载: $u" -ForegroundColor Cyan
            Invoke-WebRequest -Uri $u -OutFile $exe -UseBasicParsing -TimeoutSec 900
            if ((Get-Item $exe).Length -gt 60000000) { $ok = $true; break }
        } catch {
            Write-Host "  失败: $($_.Exception.Message)" -ForegroundColor Yellow
        }
    }
    if (-not $ok) {
        Write-Host "下载 w64devkit 失败，请手动下载后解压到 $mingw" -ForegroundColor Red
        exit 1
    }
    Write-Host "正在解压 ..." -ForegroundColor Cyan
    # w64devkit 自解压包不支持 -o 参数，解压到当前工作目录
    Push-Location $tools
    Start-Process -FilePath $exe -ArgumentList "-y" -Wait -NoNewWindow
    Pop-Location
}

# w64devkit 是纯 C 工具链，没有 libgcc_eh.a，但 rust 的 windows-gnu 目标会链接它。
# _Unwind_* 等符号实际都在 libgcc.a 中，这里补一个空归档即可满足链接器。
$gccDir = Get-ChildItem (Join-Path $mingw "lib\gcc") -Directory |
    ForEach-Object { Get-ChildItem $_.FullName -Directory } |
    Select-Object -First 1
if ($gccDir -and -not (Test-Path (Join-Path $gccDir.FullName "libgcc_eh.a"))) {
    Push-Location $gccDir.FullName
    & (Join-Path $mingw "bin\ar.exe") crs libgcc_eh.a
    Pop-Location
    Write-Host "已补充 libgcc_eh.a 占位归档" -ForegroundColor Green
}

foreach ($f in @("gcc.exe", "ar.exe", "windres.exe")) {
    if (-not (Test-Path (Join-Path $mingw "bin\$f"))) {
        Write-Host "缺少 $f，解压可能失败" -ForegroundColor Red
        exit 1
    }
}

Write-Host "正在安装 Rust windows-gnu 构建工具链 ..." -ForegroundColor Cyan
# 目标标准库（编译产物用）与 gnu 宿主工具链（编译 build script / proc-macro 用）都需要
& rustup target add x86_64-pc-windows-gnu
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

& rustup toolchain install stable-x86_64-pc-windows-gnu --profile minimal
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host ""
Write-Host "工具链就绪。执行构建：" -ForegroundColor Green
Write-Host "  powershell -ExecutionPolicy Bypass -File scripts/build.ps1 -Release"
