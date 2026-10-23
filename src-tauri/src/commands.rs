//! Tauri IPC 命令：前端通过 invoke 调用的全部接口

use std::sync::atomic::Ordering;

use tauri::{AppHandle, Manager, State};

use crate::model::{AppConfig, Config, Plan};
use crate::util::{log_line, validate_dirs};
use crate::watch::{
    emit_state, spawn_sync, start_watch, stop_watch, toast, AppState, StateEvent,
};

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub data_dir: String,
    pub log_dir: String,
}

#[tauri::command]
pub fn get_config(state: State<AppState>) -> Config {
    state.store.lock().unwrap().config.clone()
}

#[tauri::command]
pub fn save_config(app: AppHandle, state: State<AppState>, config: Config) {
    {
        let mut s = state.store.lock().unwrap();
        s.config = config;
        s.save_config();
    }
    log_line("config", "配置已保存");
    emit_state(&app);
}

#[tauri::command]
pub fn get_runtime(state: State<AppState>) -> StateEvent {
    let rt = state.runtime.lock().unwrap();
    rt.snapshot()
}

// ---------- 监听 / 同步 ----------

#[tauri::command]
pub fn start_watching(app: AppHandle) -> Result<(), String> {
    start_watch(&app)
}

#[tauri::command]
pub fn stop_watching(app: AppHandle) {
    stop_watch(&app);
}

#[tauri::command]
pub fn sync_now(app: AppHandle) -> Result<(), String> {
    let cfg = {
        let state = app.state::<AppState>();
        let s = state.store.lock().unwrap();
        s.config.current.clone()
    };
    validate_dirs(&cfg.source, &cfg.target)?;
    spawn_sync(&app, true);
    Ok(())
}

#[tauri::command]
pub fn cancel_sync(app: AppHandle) {
    let state = app.state::<AppState>();
    let rt = state.runtime.lock().unwrap();
    rt.cancel.store(true, Ordering::Relaxed);
    drop(rt);
    log_line("sync", "用户请求取消同步");
}

// ---------- 历史方案 ----------

#[tauri::command]
pub fn list_plans(state: State<AppState>) -> Vec<Plan> {
    state.store.lock().unwrap().plans.clone()
}

#[tauri::command]
pub fn save_plan(state: State<AppState>, name: Option<String>) -> Vec<Plan> {
    let mut s = state.store.lock().unwrap();
    let cur = s.config.current.clone();
    let clean = name.unwrap_or_default().trim().to_string();
    let idx = s
        .plans
        .iter()
        .position(|p| p.source == cur.source && p.target == cur.target);
    match idx {
        Some(i) => {
            let p = &mut s.plans[i];
            if !clean.is_empty() {
                p.name = clean;
            }
            p.mode = cur.mode.clone();
            p.trigger = cur.trigger.clone();
        }
        None => {
            let plan = Plan {
                id: format!("h_{}", chrono::Local::now().timestamp_millis()),
                name: if clean.is_empty() {
                    crate::model::auto_plan_name(&cur.source, &cur.target)
                } else {
                    clean
                },
                source: cur.source.clone(),
                target: cur.target.clone(),
                mode: cur.mode.clone(),
                trigger: cur.trigger.clone(),
                last_run_at: None,
                last_result: None,
                use_count: 0,
            };
            s.plans.insert(0, plan);
        }
    }
    s.trim_history();
    s.save_history();
    s.plans.clone()
}

#[tauri::command]
pub fn rename_plan(state: State<AppState>, id: String, name: String) -> Vec<Plan> {
    let mut s = state.store.lock().unwrap();
    if let Some(p) = s.plans.iter_mut().find(|p| p.id == id) {
        p.name = name.trim().to_string();
    }
    s.save_history();
    s.plans.clone()
}

#[tauri::command]
pub fn delete_plan(state: State<AppState>, id: String) -> Vec<Plan> {
    let mut s = state.store.lock().unwrap();
    s.plans.retain(|p| p.id != id);
    s.save_history();
    s.plans.clone()
}

#[tauri::command]
pub fn clear_plans(state: State<AppState>) -> Vec<Plan> {
    let mut s = state.store.lock().unwrap();
    s.plans.clear();
    s.save_history();
    s.plans.clone()
}

/// 载入方案到当前配置
#[tauri::command]
pub fn apply_plan(state: State<AppState>, id: String) -> Result<Config, String> {
    let mut s = state.store.lock().unwrap();
    let plan = s
        .plans
        .iter()
        .find(|p| p.id == id)
        .cloned()
        .ok_or_else(|| "方案不存在".to_string())?;
    s.config.current.source = plan.source;
    s.config.current.target = plan.target;
    s.config.current.mode = plan.mode;
    s.config.current.trigger = plan.trigger;
    s.save_config();
    Ok(s.config.clone())
}

/// 立即同步历史方案
#[tauri::command]
pub fn sync_plan_now(app: AppHandle, state: State<AppState>, id: String) -> Result<(), String> {
    let cfg = {
        let mut s = state.store.lock().unwrap();
        let plan = s
            .plans
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .ok_or_else(|| "方案不存在".to_string())?;
        s.config.current.source = plan.source;
        s.config.current.target = plan.target;
        s.config.current.mode = plan.mode;
        s.config.current.trigger = plan.trigger;
        s.save_config();
        s.config.clone()
    };
    validate_dirs(&cfg.current.source, &cfg.current.target)?;
    spawn_sync(&app, true);
    Ok(())
}

// ---------- 系统相关 ----------

#[tauri::command]
pub fn app_info(app: AppHandle) -> AppInfo {
    AppInfo {
        version: app.package_info().version.to_string(),
        data_dir: crate::util::data_dir().to_string_lossy().to_string(),
        log_dir: crate::util::logs_dir().to_string_lossy().to_string(),
    }
}

/// 弹出文件夹选择框（Rust 侧实现，异步命令避免阻塞主线程）
#[tauri::command]
pub async fn pick_directory(
    app: AppHandle,
    title: String,
    current: Option<String>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let dlg = app.dialog().file().set_title(title);
    let dlg = match current.filter(|c| !c.trim().is_empty()) {
        Some(c) => dlg.set_directory(c),
        None => dlg,
    };
    let picked = dlg.blocking_pick_folder();
    Ok(picked
        .and_then(|p| p.into_path().ok())
        .map(|p| p.to_string_lossy().to_string()))
}

/// 在资源管理器中打开目录（或定位到文件所在目录）
#[tauri::command]
pub fn open_path(path: String) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("路径为空".into());
    }
    let p = std::path::PathBuf::from(path.trim());
    if !p.exists() {
        return Err(format!("路径不存在：{}", p.display()));
    }
    let dir = if p.is_dir() {
        p
    } else {
        p.parent().map(|x| x.to_path_buf()).unwrap_or(p)
    };
    std::process::Command::new("explorer")
        .arg(dir)
        .spawn()
        .map_err(|e| format!("无法打开资源管理器：{e}"))?;
    Ok(())
}

/// 设置开机自启
#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    let al = app.autolaunch();
    let result = if enabled { al.enable() } else { al.disable() };
    result.map_err(|e| format!("设置开机自启失败：{e}"))?;
    al.is_enabled().map_err(|e| e.to_string())
}

/// 查询开机自启状态
#[tauri::command]
pub fn get_autostart(app: AppHandle) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}

/// 退出程序
#[tauri::command]
pub fn quit_app(app: AppHandle) {
    stop_watch(&app);
    app.exit(0);
}

/// 测试通知
#[tauri::command]
pub fn test_notify(app: AppHandle) {
    toast(&app, "文件同步助手", "这是一条测试通知");
}

/// 目录差异提示的置顶开关（前端在弹窗结束后调用 `on: false` 解除置顶）
#[tauri::command]
pub fn set_prompt_mode(app: AppHandle, on: bool) {
    crate::watch::set_prompt_mode(&app, on);
}

/// 应用配置（主题等）单独读写，便于前端分开处理
#[tauri::command]
pub fn save_app_config(state: State<AppState>, app_config: AppConfig) {
    let mut s = state.store.lock().unwrap();
    s.config.app = app_config;
    s.save_config();
}
