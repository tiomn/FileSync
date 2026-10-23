//! 文件同步助手 —— 应用入口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod model;
mod sync;
mod util;
mod watch;

use std::sync::Mutex;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, WindowEvent};
use tauri_plugin_autostart::MacosLauncher;

use model::Store;
use watch::{emit_state, show_main, start_watch, stop_watch, toast, AppState};

fn main() {
    tauri::Builder::default()
        // 单实例：重复启动时唤起已有窗口
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(AppState {
            store: Mutex::new(Store::load()),
            runtime: Mutex::new(Default::default()),
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::save_config,
            commands::save_app_config,
            commands::get_runtime,
            commands::start_watching,
            commands::stop_watching,
            commands::sync_now,
            commands::cancel_sync,
            commands::list_plans,
            commands::save_plan,
            commands::rename_plan,
            commands::delete_plan,
            commands::clear_plans,
            commands::apply_plan,
            commands::sync_plan_now,
            commands::app_info,
            commands::pick_directory,
            commands::open_path,
            commands::set_autostart,
            commands::get_autostart,
            commands::quit_app,
            commands::test_notify,
            commands::set_prompt_mode,
        ])
        .setup(|app| {
            util::prune_logs();
            util::log_line("app", "程序启动");
            // 注册通知身份（AppUserModelID），否则 Windows 会丢弃通知
            util::register_notification_identity(
                &app.config().identifier,
                "文件同步助手",
            );
            build_tray(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                let close_to_tray = {
                    let state = app.state::<AppState>();
                    let s = state.store.lock().unwrap();
                    s.config.app.close_to_tray
                };
                if close_to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                    emit_state(app);
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("FileSync 启动失败");
}

/// 托盘图标 + 右键菜单
fn build_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let open_item = MenuItem::with_id(app, "open", "打开主界面", true, None::<&str>)?;
    let sync_item = MenuItem::with_id(app, "sync", "立即同步", true, None::<&str>)?;
    let toggle_item = MenuItem::with_id(app, "toggle", "开始 / 停止监听", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open_item, &sync_item, &toggle_item, &sep, &quit_item])?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("文件同步助手")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().0.as_str() {
            "open" => show_main(app),
            "sync" => {
                // 经前端触发同步，保证「整体替换」仍走二次确认流程
                show_main(app);
                let _ = app.emit("tray://sync", ());
            }
            "toggle" => {
                let watching = {
                    let state = app.state::<AppState>();
                    let rt = state.runtime.lock().unwrap();
                    rt.watching
                };
                if watching {
                    stop_watch(app);
                } else if let Err(e) = start_watch(app) {
                    toast(app, "无法开始监听", &e);
                }
            }
            "quit" => {
                stop_watch(app);
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}
