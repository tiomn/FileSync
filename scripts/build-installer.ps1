# 生成 Windows 安装包（NSIS）
#
# 用法:
#   powershell -ExecutionPolicy Bypass -File scripts/build-installer.ps1
#
# 说明:
#   - x86_64-pc-windows-gnu 工具链无法静态链接 WebView2 加载器，程序必须与
#     WebView2Loader.dll 放在同一目录，因此安装包会一并安装该 DLL。
#   - makensis 优先使用 $env:NSIS_DIR，其次常见安装路径，都没有则下载免安装版
#     到 %LOCALAPPDATA%\FileSync-tools\nsis（无需管理员权限）。

$ErrorActionPreference = "Stop"

$root    = Split-Path -Parent $PSScriptRoot
$release = Join-Path $root "src-tauri\target\x86_64-pc-windows-gnu\release"
$stage   = Join-Path $root "dist\app"
$outFile = Join-Path $root "dist\FileSync-1.0.1-win64-Setup.exe"
$icon    = Join-Path $root "src-tauri\icons\icon.ico"
$nsi     = Join-Path $PSScriptRoot "installer.nsi"

$appExe = "FileSync.exe"
$loader = "WebView2Loader.dll"

# ---------- 1. 准备程序文件 ----------
$srcExe = Get-ChildItem $release -File -Filter "*.exe" -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -notmatch "^(uninstall|FileSync-copy)" } |
    Select-Object -First 1
$srcDll = Join-Path $release $loader

if (-not $srcExe) { Write-Host "未找到程序主体，请先执行 scripts\build.ps1" -ForegroundColor Red; exit 1 }
if (-not (Test-Path $srcDll)) { Write-Host "未找到 $loader，请先执行 scripts\build.ps1" -ForegroundColor Red; exit 1 }

New-Item -ItemType Directory -Force -Path $stage | Out-Null
Get-ChildItem $stage -File | Remove-Item -Force
Copy-Item $srcExe.FullName (Join-Path $stage $appExe) -Force
Copy-Item $srcDll (Join-Path $stage $loader) -Force
Write-Host "程序文件: $stage" -ForegroundColor Green

# ---------- 2. 定位 makensis ----------
function Find-Makensis {
    $cands = @()
    if ($env:NSIS_DIR) { $cands += (Join-Path $env:NSIS_DIR "makensis.exe") }
    $cands += "C:\Program Files (x86)\NSIS\makensis.exe"
    $cands += "C:\Program Files\NSIS\makensis.exe"
    $cands += (Join-Path $env:LOCALAPPDATA "FileSync-tools\nsis\makensis.exe")
    foreach ($c in $cands) { if (Test-Path $c) { return $c } }
    return $null
}

$makensis = Find-Makensis
if (-not $makensis) {
    $dir = Join-Path $env:LOCALAPPDATA "FileSync-tools\nsis"
    $zip = Join-Path $env:LOCALAPPDATA "FileSync-tools\nsis-3.10.zip"
    New-Item -ItemType Directory -Force -Path (Split-Path $dir) | Out-Null
    Write-Host "未检测到 NSIS，正在下载免安装版 ..." -ForegroundColor Cyan
    $urls = @(
        "https://master.dl.sourceforge.net/project/nsis/NSIS%203/3.10/nsis-3.10.zip?viasf=1",
        "https://downloads.sourceforge.net/project/nsis/NSIS%203/3.10/nsis-3.10.zip",
        "https://cfhcable.dl.sourceforge.net/project/nsis/NSIS%203/3.10/nsis-3.10.zip"
    )
    $ok = $false
    foreach ($u in $urls) {
        try {
            & curl.exe -sSL --max-time 300 -o $zip $u
            if ((Test-Path $zip) -and (Get-Item $zip).Length -gt 1000000) { $ok = $true; break }
        } catch { }
    }
    if (-not $ok) { Write-Host "下载 NSIS 失败，请手动安装后设置 `$env:NSIS_DIR" -ForegroundColor Red; exit 1 }
    Expand-Archive -Path $zip -DestinationPath (Join-Path $env:LOCALAPPDATA "FileSync-tools\nsisextract") -Force
    $inner = Get-ChildItem (Join-Path $env:LOCALAPPDATA "FileSync-tools\nsisextract") -Directory | Select-Object -First 1
    Move-Item $inner.FullName $dir -Force
    $makensis = Join-Path $dir "makensis.exe"
}
Write-Host "makensis: $makensis" -ForegroundColor Green

# ---------- 3. 编译安装包 ----------
& $makensis `
    "/DAPP_SRC=$stage" `
    "/DOUT_FILE=$outFile" `
    "/DICON_FILE=$icon" `
    $nsi
if ($LASTEXITCODE -ne 0) { Write-Host "编译失败" -ForegroundColor Red; exit $LASTEXITCODE }

Write-Host ""
Write-Host "安装包: $outFile" -ForegroundColor Green
Write-Host ("大小  : {0:N2} MB" -f ((Get-Item $outFile).Length / 1MB))

# ---------- 4. 绿色版目录（exe 必须与 WebView2Loader.dll 同目录） ----------
$green = Join-Path $root "dist\FileSync"
New-Item -ItemType Directory -Force -Path $green | Out-Null
Copy-Item (Join-Path $stage $appExe) (Join-Path $green $appExe) -Force
Copy-Item (Join-Path $stage $loader) (Join-Path $green $loader) -Force
Write-Host "绿色版: $green" -ForegroundColor Green

# ---------- 5. 清理暂存 ----------
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
