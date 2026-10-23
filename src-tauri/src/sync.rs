//! 同步引擎：差异判定 + 增量/整体同步 + 原子写入 + 失败重试

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use walkdir::WalkDir;

use crate::model::CurrentConfig;
use crate::util::{is_excluded, log_line};

/// 同步结果统计
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SyncStats {
    pub ok: bool,
    pub added: u64,
    pub updated: u64,
    pub deleted: u64,
    pub skipped: u64,
    pub failed: u64,
    pub bytes: u64,
    pub ms: u64,
    pub errors: Vec<String>,
}

impl SyncStats {
    pub fn to_last_result(&self) -> crate::model::LastResult {
        crate::model::LastResult {
            ok: self.ok,
            added: self.added,
            updated: self.updated,
            deleted: self.deleted,
            skipped: self.skipped,
            failed: self.failed,
            ms: self.ms,
        }
    }
}

/// 进度事件载荷
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub current: String,
    pub done: u64,
    pub total: u64,
    pub added: u64,
    pub updated: u64,
    pub deleted: u64,
    pub skipped: u64,
    pub failed: u64,
    pub elapsed_ms: u64,
}

const RETRY_DELAYS_MS: [u64; 3] = [300, 1000, 2000];
const HASH_LIMIT: u64 = 10 * 1024 * 1024;

/// 执行一次同步
pub fn run_sync(app: &AppHandle, cfg: &CurrentConfig, cancel: Arc<AtomicBool>) -> SyncStats {
    let started = Instant::now();
    let mut stats = SyncStats {
        ok: true,
        ..Default::default()
    };

    let source = PathBuf::from(cfg.source.trim());
    let target = PathBuf::from(cfg.target.trim());
    let mirror = cfg.mode == "mirror";

    log_line(
        "sync",
        &format!(
            "开始同步 mode={} {} -> {}",
            cfg.mode, cfg.source, cfg.target
        ),
    );

    // ---------- 0. 前置检查：来源必须在，目标不存在则创建 ----------
    if !source.is_dir() {
        stats.ok = false;
        stats.failed = 1;
        let msg = format!("监听目录不存在：{}", cfg.source.trim());
        log_line("sync", &msg);
        stats.errors.push(msg);
        stats.ms = started.elapsed().as_millis() as u64;
        emit_progress(app, Path::new(""), &stats, 0, started);
        return stats;
    }
    if let Err(e) = fs::create_dir_all(&target) {
        stats.ok = false;
        stats.failed = 1;
        let msg = format!("目标目录不存在且无法创建：{}（{e}）", cfg.target.trim());
        log_line("sync", &msg);
        stats.errors.push(msg);
        stats.ms = started.elapsed().as_millis() as u64;
        emit_progress(app, Path::new(""), &stats, 0, started);
        return stats;
    }

    // ---------- 1. 收集源目录文件清单 ----------
    let mut rels: Vec<PathBuf> = Vec::new();
    for entry in WalkDir::new(&source).follow_links(false).into_iter().flatten() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(&source) else {
            continue;
        };
        if is_excluded(rel, &cfg.excludes) {
            stats.skipped += 1;
            continue;
        }
        rels.push(rel.to_path_buf());
    }
    rels.sort();

    let total = rels.len() as u64;
    let mut last_emit = Instant::now();

    // ---------- 2. 逐个比对并复制 ----------
    for rel in &rels {
        if cancel.load(Ordering::Relaxed) {
            log_line("sync", "任务被用户取消");
            break;
        }
        let src = source.join(rel);
        let dst = target.join(rel);

        if !need_copy(&src, &dst, cfg.strict_hash) {
            stats.skipped += 1;
        } else {
            let existed = dst.exists();
            match copy_with_retry(&src, &dst, &cancel) {
                Ok(n) => {
                    stats.bytes += n;
                    if existed {
                        stats.updated += 1;
                    } else {
                        stats.added += 1;
                    }
                }
                Err(e) => {
                    stats.failed += 1;
                    let msg = format!("{} : {}", rel.display(), e);
                    log_line("sync", &format!("复制失败 {msg}"));
                    if stats.errors.len() < 50 {
                        stats.errors.push(msg);
                    }
                }
            }
        }

        if last_emit.elapsed() > Duration::from_millis(80) {
            last_emit = Instant::now();
            emit_progress(app, rel, &stats, total, started);
        }
    }

    // ---------- 3. 整体替换：清理目标多余项 ----------
    if mirror && !cancel.load(Ordering::Relaxed) {
        let mut keep: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        for rel in &rels {
            keep.insert(rel.clone());
            let mut cur = rel.parent();
            while let Some(p) = cur {
                if p.as_os_str().is_empty() {
                    break;
                }
                keep.insert(p.to_path_buf());
                cur = p.parent();
            }
        }

        let entries: Vec<_> = WalkDir::new(&target)
            .follow_links(false)
            .contents_first(true)
            .into_iter()
            .flatten()
            .collect();

        for entry in entries {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let Ok(rel) = entry.path().strip_prefix(&target) else {
                continue;
            };
            if rel.as_os_str().is_empty() || keep.contains(rel) {
                continue;
            }
            if is_excluded(rel, &cfg.excludes) {
                continue;
            }
            if entry.file_type().is_file() {
                match remove_item(entry.path(), cfg.mirror_delete_to_recycle_bin) {
                    Ok(_) => {
                        stats.deleted += 1;
                        log_line("sync", &format!("删除 {}", rel.display()));
                    }
                    Err(e) => {
                        stats.failed += 1;
                        let msg = format!("{} : 删除失败 {}", rel.display(), e);
                        log_line("sync", &msg);
                        if stats.errors.len() < 50 {
                            stats.errors.push(msg);
                        }
                    }
                }
            } else if entry.file_type().is_dir() {
                // 子项已删空，直接移除空目录
                let _ = fs::remove_dir(entry.path());
            }
        }
    }

    stats.ms = started.elapsed().as_millis() as u64;
    stats.ok = stats.failed == 0;
    log_line(
        "sync",
        &format!(
            "同步结束 新增{} 更新{} 删除{} 跳过{} 失败{} 用时{}ms",
            stats.added, stats.updated, stats.deleted, stats.skipped, stats.failed, stats.ms
        ),
    );
    emit_progress(app, Path::new(""), &stats, total, started);
    stats
}

/// 目录比对摘要（只读，不写入任何文件）
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct DiffSummary {
    pub added: u64,
    pub updated: u64,
    pub deleted: u64,
    /// 需要处理的项数合计
    pub total: u64,
}

/// 只比对两个目录，返回差异项数；不复制、不删除。
/// 判定规则与 [`run_sync`] 完全一致，避免「说有差异」却什么也没做。
pub fn diff_dirs(cfg: &CurrentConfig, cancel: &Arc<AtomicBool>) -> DiffSummary {
    let mut d = DiffSummary::default();
    let source = PathBuf::from(cfg.source.trim());
    let target = PathBuf::from(cfg.target.trim());
    let mirror = cfg.mode == "mirror";

    if !source.is_dir() {
        return d;
    }
    let _ = fs::create_dir_all(&target);

    // 源文件清单
    let mut rels: Vec<PathBuf> = Vec::new();
    for entry in WalkDir::new(&source).follow_links(false).into_iter().flatten() {
        if cancel.load(Ordering::Relaxed) {
            return d;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(&source) else {
            continue;
        };
        if is_excluded(rel, &cfg.excludes) {
            continue;
        }
        rels.push(rel.to_path_buf());
    }
    rels.sort();

    for rel in &rels {
        if cancel.load(Ordering::Relaxed) {
            return d;
        }
        let src = source.join(rel);
        let dst = target.join(rel);
        if need_copy(&src, &dst, cfg.strict_hash) {
            if dst.exists() {
                d.updated += 1;
            } else {
                d.added += 1;
            }
        }
    }

    // 整体替换模式下，目标多余文件也算差异
    if mirror {
        let mut keep: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        for rel in &rels {
            keep.insert(rel.clone());
            let mut cur = rel.parent();
            while let Some(p) = cur {
                if p.as_os_str().is_empty() {
                    break;
                }
                keep.insert(p.to_path_buf());
                cur = p.parent();
            }
        }
        for entry in WalkDir::new(&target).follow_links(false).into_iter().flatten() {
            if cancel.load(Ordering::Relaxed) {
                return d;
            }
            if !entry.file_type().is_file() {
                continue;
            }
            let Ok(rel) = entry.path().strip_prefix(&target) else {
                continue;
            };
            if rel.as_os_str().is_empty() || keep.contains(rel) {
                continue;
            }
            if is_excluded(rel, &cfg.excludes) {
                continue;
            }
            d.deleted += 1;
        }
    }

    d.total = d.added + d.updated + d.deleted;
    d
}

fn emit_progress(app: &AppHandle, rel: &Path, stats: &SyncStats, total: u64, started: Instant) {
    let p = Progress {
        current: rel.to_string_lossy().to_string(),
        done: stats.added + stats.updated + stats.skipped + stats.failed,
        total,
        added: stats.added,
        updated: stats.updated,
        deleted: stats.deleted,
        skipped: stats.skipped,
        failed: stats.failed,
        elapsed_ms: started.elapsed().as_millis() as u64,
    };
    let _ = app.emit("sync://progress", p);
}

/// 是否需要复制
fn need_copy(src: &Path, dst: &Path, strict_hash: bool) -> bool {
    let Ok(sm) = fs::metadata(src) else {
        return false;
    };
    let Ok(dm) = fs::metadata(dst) else {
        return true; // 目标不存在
    };
    if dm.is_dir() {
        return true;
    }
    if sm.len() != dm.len() {
        return true;
    }
    if strict_hash && sm.len() <= HASH_LIMIT {
        if let (Ok(a), Ok(b)) = (hash_file(src), hash_file(dst)) {
            return a != b;
        }
    }
    is_newer(sm.modified().ok(), dm.modified().ok())
}

/// 源文件是否明显比目标新（容忍 2 秒时间精度差异）
fn is_newer(a: Option<SystemTime>, b: Option<SystemTime>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a
            .duration_since(b)
            .map(|d| d > Duration::from_secs(2))
            .unwrap_or(false),
        _ => true,
    }
}

/// FNV-1a 64 位校验（避免引入额外哈希依赖）
fn hash_file(path: &Path) -> std::io::Result<u64> {
    let mut f = fs::File::open(path)?;
    let mut buf = [0u8; 64 * 1024];
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        for b in &buf[..n] {
            h ^= *b as u64;
            h = h.wrapping_mul(0x1000_0000_01b3);
        }
    }
    Ok(h)
}

/// 带重试的原子复制：先写临时文件再改名，避免产生半截文件
fn copy_with_retry(src: &Path, dst: &Path, cancel: &Arc<AtomicBool>) -> Result<u64, String> {
    let mut last_err = String::new();
    for (i, delay) in RETRY_DELAYS_MS.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err("已取消".into());
        }
        match copy_atomic(src, dst) {
            Ok(n) => return Ok(n),
            Err(e) => {
                last_err = e.to_string();
                if i + 1 < RETRY_DELAYS_MS.len() {
                    std::thread::sleep(Duration::from_millis(*delay));
                }
            }
        }
    }
    Err(last_err)
}

fn copy_atomic(src: &Path, dst: &Path) -> std::io::Result<u64> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = tmp_path_for(dst);
    let n = fs::copy(src, &tmp)?;
    // 目标已存在时先移除再改名（Windows rename 不允许覆盖）
    if dst.exists() {
        let _ = fs::remove_file(dst);
    }
    match fs::rename(&tmp, dst) {
        Ok(_) => Ok(n),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn tmp_path_for(dst: &Path) -> PathBuf {
    let name = dst
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "tmp".into());
    dst.with_file_name(format!(".{}.filesync.tmp", name))
}

/// 删除文件或目录（回收站 / 永久删除）
fn remove_item(path: &Path, to_recycle_bin: bool) -> Result<(), String> {
    if to_recycle_bin {
        trash::delete(path).map_err(|e| e.to_string())
    } else if path.is_dir() {
        fs::remove_dir_all(path).map_err(|e| e.to_string())
    } else {
        fs::remove_file(path).map_err(|e| e.to_string())
    }
}
