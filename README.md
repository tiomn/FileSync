# 文件同步助手 FileSync

Windows 桌面应用：配置「监听目录 → 目标目录」，监听目录内文件发生变更后，按选定模式
自动或手动同步到目标目录。基于 **Tauri 2**（Rust + WebView2），产物为绿色版目录或安装包。

![界面](docs/ui-mockup.html)

## 功能

| 功能 | 说明 |
|---|---|
| 目录选择 | 监听目录 / 目标目录，支持浏览选择、资源管理器打开、一键互换 |
| 目录异常处理 | 监听目录缺失（未插盘、网络盘掉线）时提示并等待重试，间隔/次数/最长等待可配置，超限自动停止监听并提醒；目标目录不存在时自动创建 |
| 启动比对 | 开始监听 / 目录恢复后立即比对两个目录，有差异弹窗询问是否立即同步，一致则静默继续 |
| 增量替换 | 仅复制新增与有变更的文件，目标目录多余文件保留 |
| 整体替换 | 镜像源目录，目标多余文件**移入回收站**（可改为永久删除），执行前提示确认 |
| 自动 / 手动同步 | 自动模式在防抖窗口后自动执行；手动模式仅提示待同步 |
| 可同步通知 | 检测到变更但尚未同步时提醒 |
| 同步完成通知 | 完成后提示新增/更新/删除/失败数量与耗时 |
| 同步失败通知 | 出错时提醒并记录日志 |
| 历史选项列表 | 保存目录对 + 模式 + 方式为方案，可载入、立即同步、重命名、删除，带上次同步结果 |
| 排除规则 | 内置 `*.tmp` `~$*` `.git` `node_modules` `Thumbs.db` 等，可编辑 |
| 托盘常驻 | 关闭窗口不退出，托盘菜单支持打开主界面、立即同步、启停监听、退出 |
| 开机自启 | 设置中开关 |
| 严格校验 | 可选对小于 10MB 的文件比对内容哈希 |
| 主题 | 跟随系统 / 浅色 / 深色 |

## 目录结构

```
filesSync/
├─ dist/            发布产物（安装包 + 绿色版）
├─ docs/            技术方案与界面稿
├─ scripts/         构建脚本（含工具链一键准备、安装包打包）
├─ src/             前端（原生 HTML/CSS/JS，无构建步骤）
│  ├─ index.html
│  ├─ styles.css
│  └─ app.js
└─ src-tauri/       Rust 后端
   ├─ src/main.rs       入口、托盘、窗口事件
   ├─ src/commands.rs   IPC 接口
   ├─ src/model.rs      配置与历史方案持久化
   ├─ src/watch.rs      监听 + 防抖 + 调度状态机
   ├─ src/sync.rs       同步引擎
   └─ src/util.rs       工具函数
```

## 构建

本机无需安装 Visual Studio，使用便携版 mingw-w64 + Rust `x86_64-pc-windows-gnu`。

```powershell
# 1. 一次性准备工具链（下载 w64devkit，约 64MB，无需管理员）
#    同时安装 rust 目标 x86_64-pc-windows-gnu 与 gnu 宿主工具链
powershell -ExecutionPolicy Bypass -File scripts/setup-toolchain.ps1

# 2. 构建 exe
powershell -ExecutionPolicy Bypass -File scripts/build.ps1 -Release
# 产物: src-tauri/target/x86_64-pc-windows-gnu/release/filesync.exe
```

> **为什么需要 gnu 宿主工具链**：`--target x86_64-pc-windows-gnu` 只影响最终产物，
> build script 与 proc-macro 仍按**宿主**目标编译。默认宿主是 msvc，在没装 MSVC 的机器上
> 会报 `linker 'link.exe' not found`。因此 `build.ps1` 用
> `cargo +stable-x86_64-pc-windows-gnu`，让宿主产物也用 gcc 链接。
>
> 两个 `.ps1` 脚本必须保存为 **UTF-8 with BOM**，否则 Windows PowerShell 5.1 会按 GBK
> 解码中文注释，导致后续语句被吞掉。

> 若已安装 Visual Studio 的 C++ 工作负载，也可直接用 msvc 目标：
> `cargo build --release --manifest-path src-tauri/Cargo.toml`
> （msvc 会静态链接 WebView2 加载器，得到真正的单文件 exe）

## 打包与发布

```powershell
# 生成安装包（依赖上面已构建的 exe）
powershell -ExecutionPolicy Bypass -File scripts/build-installer.ps1
```

| 产物 | 路径 | 说明 |
|---|---|---|
| 安装包 | `dist/FileSync-1.0.1-win64-Setup.exe` | NSIS 单文件安装包（约 2.5MB），**当前用户安装，无需管理员**，自动写入开始菜单/桌面快捷方式与「应用和功能」卸载项；WebView2 运行时缺失时会提示 |
| 绿色版 | `dist/FileSync/` | `FileSync.exe` + `WebView2Loader.dll`，解压即用 |

> **为什么绿色版不止一个文件**：`x86_64-pc-windows-gnu` 工具链下
> `webview2-com-sys` 只能链接动态加载器（只有 `target_env = "msvc"` 才静态链接
> `WebView2LoaderStatic.lib`），因此 **`FileSync.exe` 必须与 `WebView2Loader.dll`
> 放在同一目录**，单独把 exe 拷到别处会卡在启动阶段。安装包会自动处理这一点。
>
> 安装包默认安装到 `%LOCALAPPDATA%\Programs\FileSync`，卸载时可选是否一并删除配置与历史。
> `scripts/build-installer.ps1` 会自动下载免安装版 NSIS 到 `%LOCALAPPDATA%\FileSync-tools\nsis`
> （也可用 `$env:NSIS_DIR` 指定已有的 NSIS）。

### 版本号规则

版本形如 `大版本 . 中版本 . 小版本`，**每次功能调整都必须升版本**：

1. 小版本号 +1（`1.0.1` → `1.0.2` …）；
2. 小版本号到达 **150** 时，中版本号 +1，小版本号归零（`1.0.150` → `1.1.0`）；
3. 中版本号到达 **10** 时，大版本号 +1，中版本号与小版本号归零（`1.10.0` → `2.0.0`）。

> 注意顺序：小版本先到 150 → 中版本 +1；中版本到 10 → 大版本 +1。
> 需要同步修改的地方：`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`、
> `scripts/installer.nsi`（`APP_VERSION` / `VIProductVersion` / `OUT_FILE`）、
> `scripts/build-installer.ps1` 的输出文件名、`src/index.html` 里的静态回退版本号。

## 运行环境

- Windows 10 / 11（x64）
- WebView2 Runtime（Win11 与较新 Win10 已内置）

## 数据位置

| 内容 | 路径 |
|---|---|
| 配置 | `%APPDATA%\FileSync\config.json` |
| 历史方案 | `%APPDATA%\FileSync\history.json` |
| 日志 | `%APPDATA%\FileSync\logs\sync-YYYYMMDD.log`（保留 7 天） |
| 通知图标 | `%APPDATA%\FileSync\notify-icon.png` |
| 界面缓存 | `%LOCALAPPDATA%\FileSync\EBWebView`（可随时删除） |

> 配置文件可手动编辑；即使被记事本保存成 **UTF-8 with BOM** 也能正常读取。
> 程序会在 `HKCU\Software\Classes\AppUserModelId\FileSync` 注册系统通知身份，
> 否则 Windows 会静默丢弃通知气泡。

## 同步规则

复制条件（满足其一即复制）：

1. 目标不存在
2. 文件大小不同
3. 源修改时间比目标新超过 2 秒（容忍时间精度差异）
4. 开启严格校验且内容哈希不同

其他细节：

- 原子写入：先写 `.文件名.filesync.tmp` 再改名，避免产生半截文件
- 被占用/写入中的文件会退避重试（0.3 / 1 / 2 秒，共 3 次）
- 变更事件经过防抖合并，只处理最终状态
- 同步期间产生的新变更会排队，本轮结束后按自动模式继续
- 目标目录位于监听目录内部、监听与目标相同等情况会在启动时直接拒绝（目标目录缺失时先自动创建再判断）

## 目录异常处理

监听目录可能因为 U 盘未插、网络盘掉线、外置硬盘休眠等原因**暂时不存在**。
此时程序不会直接报错退出，而是：

1. 立即弹出通知，并在界面顶部显示黄色提示条；
2. 按设置的**重试间隔**反复探测（每轮都会刷新「N 秒后重试（第 i / 上限 次）」）；
3. 目录重新出现后自动恢复监听，并立即比对两个目录（见下）；
4. 超过**最大重试次数**或**最长等待时间**后，自动停止监听并弹出红色提示，
   需检查目录后手动重新开始监听（在提示条上点「取消监听」可提前放弃）。

| 配置项 | 默认值 | 说明 |
|---|---|---|
| 重试间隔 | 10 秒 | 每轮探测的等待时间 |
| 最大重试次数 | 30 | `0` 表示不限次数 |
| 最长等待 | 300 秒 | `0` 表示不限时长 |

> 监听过程中目录消失（例如拔掉 U 盘）同样会转入上述等待重试流程。
> 手动「立即同步」时若监听目录不存在，会直接提示错误，不会等待。

## 启动 / 恢复时的目录比对

点「开始监听」时，以及监听目录从缺失状态恢复后，程序会**立即比对**两个目录：

| 比对结果 | 行为 |
|---|---|
| 完全一致 | 不做任何操作，继续监听 |
| 存在差异 | 弹窗「发现目录差异」，列出新增 / 更新 / 删除项数，询问是否立即同步 |

- 选「立即同步」→ 按当前模式执行一次同步（整体替换模式会在弹窗里额外说明会被移入回收站的项数）
- 选「暂不同步」→ 关闭弹窗，仅继续监听；之后有新变更仍按「自动 / 手动」方式处理
- 判定规则与同步引擎完全一致，不会出现「说有差异却什么也没做」
- 扫描本身导致的目录时间戳变化（notify 会报成目录 `Modify` 事件）会被过滤，不会误触发一次同步
- 弹窗**无视当前窗口**：会唤出主窗口并临时置顶（即使收在托盘、或正被其它程序遮挡），作答后自动取消置顶
