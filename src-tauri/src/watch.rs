//! 监听 + 调度：文件变更监听、防抖合并、自动/手动同步状态机
//!
//! 监听目录不存在时不会直接失败，而是进入「等待重试」：
//! 按配置的间隔反复探测，超过次数或总时长后自动停止监听并提示。
//! 监听过程中目录消失（U 盘拔出、网络盘断开等）同样转入等待重试。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::model::{CurrentConfig, Store};
use crate::sync::{diff_dirs, run_sync, SyncStats};
use crate::util::{log_line, validate_pair};

/// 监听健康检查间隔（事件循环空闲时也会定期确认目录是否还在）
const HEALTH_CHECK: Duration = Duration::from_secs(5);

/// 运行时状态
pub struct Runtime {
    /// 用户意图：监听会话是否开启（等待重试期间仍为 true）
    pub watching: bool,
    pub syncing: bool,
    pub pending: bool,
    pub pending_count: u64,
    pub cancel: Arc<AtomicBool>,
    /// 监听线程的中止信号
    pub stop: Option<Arc<AtomicBool>>,
    /// 监听会话代号：每次 start/stop 递增，用于让旧线程失效
    pub generation: u64,
    /// 正在等待监听目录恢复
    pub waiting: bool,
    pub retry_attempt: u64,
    pub retry_max: u64,
    pub retry_timeout_secs: u64,
    /// 距下次重试的剩余秒数
    pub retry_in_secs: u64,
    pub wait_reason: Option<String>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            watching: false,
            syncing: false,
            pending: false,
            pending_count: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            stop: None,
            generation: 0,
            waiting: false,
            retry_attempt: 0,
            retry_max: 0,
            retry_timeout_secs: 0,
            retry_in_secs: 0,
            wait_reason: None,
        }
    }
}

/// 全局状态
pub struct AppState {
    pub store: Mutex<Store>,
    pub runtime: Mutex<Runtime>,
}

/// 推送给前端的运行状态
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct StateEvent {
    pub watching: bool,
    pub syncing: bool,
    pub pending: bool,
    pub pending_count: u64,
    pub waiting: bool,
    pub retry_attempt: u64,
    pub retry_max: u64,
    pub retry_timeout_secs: u64,
    pub retry_in_secs: u64,
    pub wait_reason: Option<String>,
}

impl Runtime {
    pub fn snapshot(&self) -> StateEvent {
        StateEvent {
            watching: self.watching,
            syncing: self.syncing,
            pending: self.pending,
            pending_count: self.pending_count,
            waiting: self.waiting,
            retry_attempt: self.retry_attempt,
            retry_max: self.retry_max,
            retry_timeout_secs: self.retry_timeout_secs,
            retry_in_secs: self.retry_in_secs,
            wait_reason: self.wait_reason.clone(),
        }
    }
}

pub fn emit_state(app: &AppHandle) {
    let state = app.state::<AppState>();
    let payload = {
        let rt = state.runtime.lock().unwrap();
        rt.snapshot()
    };
    let _ = app.emit("sync://state", payload);
}

/// 系统通知（失败静默降级）
pub fn toast(app: &AppHandle, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        log_line("notify", &format!("通知发送失败: {e}"));
    }
}

// ---------------------------------------------------------------- 监听会话

/// 一次监听会话的运行参数快照
#[derive(Clone)]
struct WatchParams {
    source: PathBuf,
    debounce_ms: u64,
    retry_interval_secs: u64,
    retry_max: u64,
    retry_timeout_secs: u64,
}

type WatchRx = mpsc::Receiver<notify::Result<notify::Event>>;

enum WaitOutcome {
    /// 可以再试一次
    Continue,
    /// 超过配置的上限，放弃
    GiveUp,
    /// 会话已被停止 / 被新会话取代
    Stopped,
}

/// 开始监听
pub fn start_watch(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let cfg = {
        let s = state.store.lock().unwrap();
        s.config.current.clone()
    };
    // 目标目录不存在会自动创建；来源目录缺失不报错，交给监听线程等待重试
    validate_pair(&cfg.source, &cfg.target)?;

    stop_watch(app);

    let params = WatchParams {
        source: PathBuf::from(cfg.source.trim()),
        debounce_ms: cfg.debounce_ms.clamp(200, 60_000),
        retry_interval_secs: cfg.retry_interval_secs.clamp(1, 86_400),
        retry_max: cfg.retry_max.min(100_000),
        retry_timeout_secs: cfg.retry_timeout_secs.min(86_400),
    };

    let stop = Arc::new(AtomicBool::new(false));
    let gen = {
        let mut rt = state.runtime.lock().unwrap();
        rt.generation = rt.generation.wrapping_add(1);
        rt.stop = Some(stop.clone());
        rt.watching = true;
        rt.pending = false;
        rt.pending_count = 0;
        rt.waiting = false;
        rt.retry_attempt = 0;
        rt.retry_max = params.retry_max;
        rt.retry_timeout_secs = params.retry_timeout_secs;
        rt.retry_in_secs = 0;
        rt.wait_reason = None;
        rt.generation
    };
    emit_state(app);

    let app_thread = app.clone();
    let stop_thread = stop.clone();
    std::thread::spawn(move || {
        supervisor(&app_thread, &params, &stop_thread, gen);
        log_line("watch", "监听线程已退出");
    });

    log_line("watch", &format!("开启监听会话 {}", cfg.source));
    Ok(())
}

/// 停止监听
pub fn stop_watch(app: &AppHandle) {
    let state = app.state::<AppState>();
    {
        let mut rt = state.runtime.lock().unwrap();
        if let Some(stop) = rt.stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        // 代号递增，保证旧线程后续的状态写入被丢弃
        rt.generation = rt.generation.wrapping_add(1);
        rt.watching = false;
        rt.pending = false;
        rt.pending_count = 0;
        rt.waiting = false;
        rt.retry_attempt = 0;
        rt.retry_in_secs = 0;
        rt.wait_reason = None;
    }
    emit_state(app);
}

/// 监听线程主循环：等待目录 → 建立监听 → 收到事件
fn supervisor(app: &AppHandle, p: &WatchParams, stop: &Arc<AtomicBool>, gen: u64) {
    // 连续失败次数；目录恢复后清零
    let mut attempt: u64 = 0;
    // 本轮缺失的起点，用于计算总等待时长
    let mut missing_since: Option<Instant> = None;
    // 是否因「目录不存在」而等待（区别于 watcher 建立失败）
    let mut dir_missing = false;
    // 本会话是否已经成功建立过监听（用于判断「开始监听」后的首次比对）
    let mut ever_up = false;

    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }

        // ---------- 阶段 1：目录是否可用 ----------
        if !p.source.is_dir() {
            dir_missing = true;
            let reason = format!("监听目录不存在：{}", p.source.display());
            match wait_once(app, p, stop, gen, &reason, &mut attempt, &mut missing_since) {
                WaitOutcome::Continue => continue,
                WaitOutcome::Stopped => return,
                WaitOutcome::GiveUp => {
                    give_up(app, p, attempt);
                    return;
                }
            }
        }

        // ---------- 阶段 2：建立监听 ----------
        let (watcher, rx) = match build_watcher(&p.source) {
            Ok(v) => v,
            Err(e) => {
                let reason = format!("无法监听目录：{e}");
                match wait_once(app, p, stop, gen, &reason, &mut attempt, &mut missing_since) {
                    WaitOutcome::Continue => continue,
                    WaitOutcome::Stopped => return,
                    WaitOutcome::GiveUp => {
                        give_up(app, p, attempt);
                        return;
                    }
                }
            }
        };

        let recovered = dir_missing;
        let first_up = !ever_up;
        ever_up = true;
        dir_missing = false;
        missing_since = None;
        attempt = 0;
        if !publish(app, gen, |rt| {
            rt.watching = true;
            rt.waiting = false;
            rt.retry_attempt = 0;
            rt.retry_in_secs = 0;
            rt.wait_reason = None;
        }) {
            return;
        }
        emit_state(app);
        log_line("watch", &format!("开始监听 {}", p.source.display()));

        if recovered {
            toast(
                app,
                "监听已恢复",
                &format!("目录重新可用，继续监听：{}", p.source.display()),
            );
        }

        // 开始监听 / 目录恢复后：立刻比对两个目录，有差异才请前端弹窗询问是否同步
        if first_up || recovered {
            spawn_diff_check(app, gen, stop.clone());
        }

        // ---------- 阶段 3：处理文件事件 ----------
        let reason = event_loop(app, p, stop, gen, &rx);
        drop(watcher);
        if stop.load(Ordering::Relaxed) || !is_current(app, gen) {
            return;
        }

        let reason = reason.unwrap_or_else(|| "监听通道已断开".into());
        log_line("watch", &format!("{reason}，转入等待重试"));

        // 立即置为等待态，让界面先有反馈
        let reason_text = reason.clone();
        if !publish(app, gen, |rt| {
            rt.waiting = true;
            rt.wait_reason = Some(reason_text);
        }) {
            return;
        }
        emit_state(app);

        // 冷却后重试（目录已回来但监听器出问题时同样需要退避）
        match wait_once(app, p, stop, gen, &reason, &mut attempt, &mut missing_since) {
            WaitOutcome::Continue => continue,
            WaitOutcome::Stopped => return,
            WaitOutcome::GiveUp => {
                give_up(app, p, attempt);
                return;
            }
        }
    }
}

/// 建立监听器
fn build_watcher(source: &Path) -> Result<(notify::RecommendedWatcher, WatchRx), String> {
    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(tx).map_err(|e| e.to_string())?;
    watcher
        .watch(source, RecursiveMode::Recursive)
        .map_err(|e| e.to_string())?;
    Ok((watcher, rx))
}

/// 事件循环；返回中断原因（None 表示被停止）
fn event_loop(
    app: &AppHandle,
    p: &WatchParams,
    stop: &Arc<AtomicBool>,
    gen: u64,
    rx: &WatchRx,
) -> Option<String> {
    let mut pending: u64 = 0;
    let mut last_check = Instant::now();
    let mut ev_count: u32 = 0;

    loop {
        if stop.load(Ordering::Relaxed) {
            return None;
        }
        match rx.recv_timeout(Duration::from_millis(p.debounce_ms)) {
            Ok(Ok(ev)) => {
                if ev.paths.is_empty() {
                    continue;
                }
                // 目录自身的元数据变更（修改时间 / 访问时间）不代表内容变化，
                // 常见于我们自己的目录扫描、杀毒软件扫描等，忽略以免误触发同步。
                if is_dir_metadata_event(&ev) {
                    ev_count += 1;
                    if ev_count <= 5 {
                        log_line(
                            "watch",
                            &format!("忽略目录元数据事件 {:?}", ev.paths),
                        );
                    }
                    continue;
                }
                pending += ev.paths.len() as u64;
            }
            Ok(Err(e)) => {
                log_line("watch", &format!("监听错误: {e}"));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if pending > 0 {
                    let n = pending;
                    pending = 0;
                    if stop.load(Ordering::Relaxed) {
                        return None;
                    }
                    // 目录已消失时不要触发同步（否则只会得到一次「配置无效」）
                    if !p.source.is_dir() {
                        return Some(format!("监听目录已消失：{}", p.source.display()));
                    }
                    handle_change(app, n);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Some("监听通道已断开".into());
            }
        }

        // 定期确认目录还在（U 盘 / 网络盘掉线时 notify 不一定报错）
        if last_check.elapsed() >= HEALTH_CHECK {
            last_check = Instant::now();
            if !is_current(app, gen) {
                return None;
            }
            if !p.source.is_dir() {
                return Some(format!("监听目录已消失：{}", p.source.display()));
            }
        }
    }
}

/// 是否是「仅涉及目录的元数据变更」事件
fn is_dir_metadata_event(ev: &notify::Event) -> bool {
    use notify::event::ModifyKind;
    use notify::EventKind;
    match ev.kind {
        // 重命名等结构变化要保留
        EventKind::Modify(ModifyKind::Name(_)) => false,
        EventKind::Modify(_) => !ev.paths.is_empty() && ev.paths.iter().all(|p| p.is_dir()),
        _ => false,
    }
}

/// 等待一次后重试；超限返回 [`WaitOutcome::GiveUp`]
fn wait_once(
    app: &AppHandle,
    p: &WatchParams,
    stop: &Arc<AtomicBool>,
    gen: u64,
    reason: &str,
    attempt: &mut u64,
    missing_since: &mut Option<Instant>,
) -> WaitOutcome {
    let since = *missing_since.get_or_insert_with(Instant::now);
    *attempt += 1;

    let over_count = p.retry_max > 0 && *attempt > p.retry_max;
    let over_time = p.retry_timeout_secs > 0 && since.elapsed().as_secs() >= p.retry_timeout_secs;
    if over_count || over_time {
        return WaitOutcome::GiveUp;
    }

    let interval = p.retry_interval_secs.max(1);
    let attempt_now = *attempt;
    let reason_owned = reason.to_string();
    if !publish(app, gen, |rt| {
        rt.watching = true;
        rt.waiting = true;
        rt.retry_attempt = attempt_now;
        rt.retry_max = p.retry_max;
        rt.retry_timeout_secs = p.retry_timeout_secs;
        rt.retry_in_secs = interval;
        rt.wait_reason = Some(reason_owned.clone());
    }) {
        return WaitOutcome::Stopped;
    }
    emit_state(app);

    if attempt_now == 1 {
        let limit = retry_limit_text(p);
        log_line(
            "watch",
            &format!("{reason}；开始重试（每 {interval} 秒一次{limit}）"),
        );
        toast(
            app,
            "监听目录不可用",
            &format!("{reason}\n将每 {interval} 秒重试一次{limit}"),
        );
    }

    let mut left = interval;
    while left > 0 {
        if stop.load(Ordering::Relaxed) {
            return WaitOutcome::Stopped;
        }
        std::thread::sleep(Duration::from_secs(1));
        left -= 1;
        if !publish(app, gen, |rt| rt.retry_in_secs = left) {
            return WaitOutcome::Stopped;
        }
        emit_state(app);
    }
    WaitOutcome::Continue
}

/// 超过重试上限：自动停止监听并提示异常
fn give_up(app: &AppHandle, p: &WatchParams, attempt: u64) {
    // attempt 在判定前已 +1，这里还原为「实际重试过的次数」
    let done = attempt.saturating_sub(1);
    let msg = format!(
        "监听目录持续不可用，已自动停止监听（重试 {done} 次）：{}",
        p.source.display()
    );
    log_line("watch", &msg);
    stop_watch(app);
    toast(app, "已自动停止监听", &msg);
    let _ = app.emit(
        "sync://alert",
        serde_json::json!({ "kind": "watchStopped", "message": msg }),
    );
}

/// 开始监听 / 目录恢复后，比对两个目录并把差异推给前端，
/// 由前端弹窗询问「是否立即同步」。内容一致时什么也不做。
fn spawn_diff_check(app: &AppHandle, gen: u64, stop: Arc<AtomicBool>) {
    let app_thread = app.clone();
    std::thread::spawn(move || {
        let state = app_thread.state::<AppState>();
        let cfg = {
            let s = state.store.lock().unwrap();
            let rt = state.runtime.lock().unwrap();
            if rt.syncing || rt.generation != gen || !rt.watching || rt.waiting {
                return;
            }
            s.config.current.clone()
        };

        let d = diff_dirs(&cfg, &stop);
        if stop.load(Ordering::Relaxed) || !is_current(&app_thread, gen) {
            return;
        }
        log_line(
            "watch",
            &format!(
                "目录比对：新增 {} 更新 {} 删除 {}",
                d.added, d.updated, d.deleted
            ),
        );
        if d.total == 0 {
            log_line("watch", "两个目录内容一致，继续监听");
            return;
        }

        // 这次提示必须让用户看到：唤出窗口并临时置顶（用户回答后由前端解除）
        set_prompt_mode(&app_thread, true);
        std::thread::sleep(Duration::from_millis(150));
        let _ = app_thread.emit("sync://diff", d);
    });
}

fn retry_limit_text(p: &WatchParams) -> String {
    let mut parts: Vec<String> = Vec::new();
    if p.retry_max > 0 {
        parts.push(format!("最多 {} 次", p.retry_max));
    }
    if p.retry_timeout_secs > 0 {
        parts.push(format!("最长 {} 秒", p.retry_timeout_secs));
    }
    if parts.is_empty() {
        "，不限次数与时长".to_string()
    } else {
        format!("，{}", parts.join("、"))
    }
}

/// 带代号校验地写入运行时状态；返回 false 表示本会话已失效
fn publish<F>(app: &AppHandle, gen: u64, f: F) -> bool
where
    F: FnOnce(&mut Runtime),
{
    let state = app.state::<AppState>();
    let mut rt = state.runtime.lock().unwrap();
    if rt.generation != gen {
        return false;
    }
    f(&mut rt);
    true
}

fn is_current(app: &AppHandle, gen: u64) -> bool {
    let state = app.state::<AppState>();
    let rt = state.runtime.lock().unwrap();
    rt.generation == gen
}

// ---------------------------------------------------------------- 同步调度

/// 变更处理：自动同步或置为待同步
fn handle_change(app: &AppHandle, count: u64) {
    let state = app.state::<AppState>();
    let (trigger, notify_pending, syncing) = {
        let s = state.store.lock().unwrap();
        let rt = state.runtime.lock().unwrap();
        (
            s.config.current.trigger.clone(),
            s.config.notify.on_pending,
            rt.syncing,
        )
    };
    if syncing {
        // 正在同步中产生的变更，排队等本轮结束后再触发
        let mut rt = state.runtime.lock().unwrap();
        rt.pending = true;
        rt.pending_count += count;
        drop(rt);
        log_line("watch", &format!("同步中收到变更 {count} 项，排队"));
        return;
    }
    log_line(
        "watch",
        &format!("处理变更 {count} 项（trigger={trigger}）"),
    );
    if trigger == "auto" {
        spawn_sync(app, false);
    } else {
        let mut rt = state.runtime.lock().unwrap();
        rt.pending = true;
        rt.pending_count += count;
        drop(rt);
        emit_state(app);
        if notify_pending {
            toast(
                app,
                "检测到文件变更",
                &format!("约有 {count} 项变化待同步，点击「立即同步」开始"),
            );
        }
        log_line("watch", &format!("检测到变更 {count} 项，等待手动同步"));
    }
}

/// 触发一次同步（在后台线程执行）
pub fn spawn_sync(app: &AppHandle, manual: bool) {
    log_line("sync", &format!("触发同步 manual={manual}"));
    let state = app.state::<AppState>();
    {
        let mut rt = state.runtime.lock().unwrap();
        if rt.syncing {
            rt.pending = true;
            return;
        }
        rt.syncing = true;
        rt.pending = false;
        rt.pending_count = 0;
        rt.cancel = Arc::new(AtomicBool::new(false));
    }
    emit_state(app);

    let app_thread = app.clone();
    std::thread::spawn(move || {
        let state = app_thread.state::<AppState>();
        let cfg: CurrentConfig = {
            let s = state.store.lock().unwrap();
            s.config.current.clone()
        };
        let cancel = {
            let rt = state.runtime.lock().unwrap();
            rt.cancel.clone()
        };

        if let Err(e) = crate::util::validate_dirs(&cfg.source, &cfg.target) {
            log_line("sync", &format!("配置无效: {e}"));
            finish(&app_thread, &e, None, manual);
            return;
        }

        let stats = run_sync(&app_thread, &cfg, cancel);

        // 回写历史方案与上次结果
        {
            let mut s = state.store.lock().unwrap();
            s.upsert_plan_from_current(Some(stats.to_last_result()));
        }

        let body = format!(
            "新增 {} · 更新 {} · 删除 {} · 跳过 {} · 失败 {} · 用时 {:.1} 秒",
            stats.added,
            stats.updated,
            stats.deleted,
            stats.skipped,
            stats.failed,
            stats.ms as f64 / 1000.0
        );
        finish(&app_thread, &body, Some(stats), manual);
    });
}

/// 收尾：更新状态 + 推送事件 + 通知
fn finish(app: &AppHandle, message: &str, stats: Option<SyncStats>, _manual: bool) {
    let state = app.state::<AppState>();
    let (on_complete, on_error) = {
        let s = state.store.lock().unwrap();
        (s.config.notify.on_complete, s.config.notify.on_error)
    };

    let ok = stats.as_ref().map(|s| s.ok).unwrap_or(false);
    let had_pending = {
        let mut rt = state.runtime.lock().unwrap();
        rt.syncing = false;
        let had = rt.pending;
        if had {
            rt.pending = false;
        }
        had
    };
    emit_state(app);

    let _ = app.emit(
        "sync://done",
        serde_json::json!({
            "ok": ok,
            "message": message,
            "stats": stats,
        }),
    );

    if stats.is_some() {
        if ok && on_complete {
            toast(app, "同步完成", message);
        } else if !ok && on_error {
            toast(app, "同步出现问题", message);
        }
    } else if on_error {
        toast(app, "同步无法执行", message);
    }

    // 同步期间又产生了新变更，若为自动模式则继续
    if had_pending {
        let trigger = {
            let s = state.store.lock().unwrap();
            s.config.current.trigger.clone()
        };
        if trigger == "auto" {
            spawn_sync(app, false);
        }
    }
}

/// 主窗口显示到前台（托盘菜单用）
pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// 目录差异提示的前置 / 收尾处理：
/// `on = true` 时唤出主窗口并临时置顶，保证提示压在所有窗口最上层；
/// `on = false` 时解除置顶。前端在用户作答（或跳过提示）后必须调用一次解除。
pub fn set_prompt_mode(app: &AppHandle, on: bool) {
    if let Some(w) = app.get_webview_window("main") {
        if on {
            let _ = w.show();
            let _ = w.unminimize();
            let _ = w.set_always_on_top(true);
            let _ = w.set_focus();
        } else {
            let _ = w.set_always_on_top(false);
        }
    }
}
