//! 通用工具：数据目录、日志、glob 匹配、时间格式化

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::Local;

/// 应用数据目录：%APPDATA%\FileSync（含 logs 子目录）
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("FileSync");
    let _ = fs::create_dir_all(&dir);
    let _ = fs::create_dir_all(dir.join("logs"));
    dir
}

pub fn logs_dir() -> PathBuf {
    let d = data_dir().join("logs");
    let _ = fs::create_dir_all(&d);
    d
}

/// 人类可读时间：2026-10-19 14:22:31
pub fn now_text() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// ISO 时间：2026-10-19T14:22:31（用于 JSON 存储）
pub fn now_iso() -> String {
    Local::now().format("%Y-%m-%dT%H:%M:%S").to_string()
}

static LOG_LOCK: Mutex<()> = Mutex::new(());

/// 滚动日志：logs/sync-YYYYMMDD.log，保留最近 7 天
pub fn log_line(scope: &str, msg: &str) {
    let _guard = LOG_LOCK.lock();
    let file = logs_dir().join(format!("sync-{}.log", Local::now().format("%Y%m%d")));
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&file) {
        let _ = writeln!(f, "[{}] [{}] {}", now_text(), scope, msg);
    }
}

/// 清理 7 天前的日志文件
pub fn prune_logs() {
    let cutoff = Local::now().date_naive() - chrono::Duration::days(7);
    if let Ok(rd) = fs::read_dir(logs_dir()) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let stamp = name.trim_start_matches("sync-").trim_end_matches(".log");
            if let Ok(d) = chrono::NaiveDate::parse_from_str(stamp, "%Y%m%d") {
                if d < cutoff {
                    let _ = fs::remove_file(e.path());
                }
            }
        }
    }
}

/// 通配符匹配（大小写不敏感，Windows 语义）：支持 `*` 与 `?`
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    // 迭代 + 回溯，避免递归
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = pi;
            mark = ti;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// 路径是否命中排除规则（对每一级路径分量与文件名做匹配）
pub fn is_excluded(rel: &std::path::Path, excludes: &[String]) -> bool {
    if excludes.is_empty() {
        return false;
    }
    let name = rel
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    for comp in rel.components() {
        let c = comp.as_os_str().to_string_lossy().to_string();
        if c.is_empty() {
            continue;
        }
        for pat in excludes {
            let pat = pat.trim();
            if pat.is_empty() {
                continue;
            }
            if glob_match(pat, &c) || glob_match(pat, &name) {
                return true;
            }
        }
    }
    false
}

/// 注册 AppUserModelID，让未安装（绿色版）程序也能弹出系统通知。
/// Windows 只会显示来自已注册 AUMID 的 toast，未注册时通知会被静默丢弃。
/// 注意：IconUri 必须是图片文件，指向 exe 会让整个注册被忽略、通知里只显示 AUMID。
pub fn register_notification_identity(aumid: &str, display_name: &str) {
    #[cfg(windows)]
    {
        use windows_registry::CURRENT_USER;

        const ICON_PNG: &[u8] = include_bytes!("../icons/32x32.png");
        let icon = data_dir().join("notify-icon.png");
        if !icon.exists() {
            let _ = fs::write(&icon, ICON_PNG);
        }

        let path = format!(r"SOFTWARE\Classes\AppUserModelId\{aumid}");
        match CURRENT_USER.create(&path) {
            Ok(key) => {
                let _ = key.set_string("DisplayName", display_name);
                let _ = key.set_string("IconBackgroundColor", "0");
                let _ = key.set_string("IconUri", icon.to_string_lossy().as_ref());
            }
            Err(e) => log_line("notify", &format!("注册通知标识失败: {e}")),
        }
    }
    #[cfg(not(windows))]
    let _ = (aumid, display_name);
}

/// 结构性校验目录配置。
///
/// 与「目录是否存在」无关：目标目录不存在时**自动创建**，来源目录缺失不报错
/// （由监听线程决定是否进入等待重试）。
pub fn validate_pair(source: &str, target: &str) -> Result<(), String> {
    let s = source.trim();
    let t = target.trim();
    if s.is_empty() {
        return Err("请先选择监听目录".into());
    }
    if t.is_empty() {
        return Err("请先选择目标目录".into());
    }

    let sp = std::path::Path::new(s);
    let tp = std::path::Path::new(t);

    // 目标目录不存在则自动创建（含多级父目录）
    if !tp.exists() {
        fs::create_dir_all(tp)
            .map_err(|e| format!("目标目录不存在且无法创建：{t}（{e}）"))?;
        log_line("dir", &format!("目标目录不存在，已自动创建：{t}"));
    } else if !tp.is_dir() {
        return Err(format!("目标路径已存在但不是目录：{t}"));
    }

    let cs = sp.canonicalize().unwrap_or_else(|_| sp.to_path_buf());
    let ct = tp.canonicalize().unwrap_or_else(|_| tp.to_path_buf());
    if cs == ct {
        return Err("目标目录不能与监听目录相同".into());
    }
    if ct.starts_with(&cs) {
        return Err("目标目录不能位于监听目录内部（会造成递归同步）".into());
    }
    Ok(())
}

/// 完整校验：在结构校验之外，还要求监听目录此刻可用。
/// 用于「立即同步」这类用户主动触发的动作。
pub fn validate_dirs(source: &str, target: &str) -> Result<(), String> {
    validate_pair(source, target)?;
    let s = std::path::Path::new(source.trim());
    if !s.is_dir() {
        return Err(format!("监听目录不存在或不可访问：{}", source.trim()));
    }
    Ok(())
}
