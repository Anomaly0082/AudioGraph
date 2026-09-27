#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod backend;
mod graph_files;
mod ai;
mod ai_commands;
mod ai_settings;

use std::sync::{Arc, atomic::Ordering};
use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

fn shutdown_for_close(app: tauri::AppHandle, manager: Arc<backend::BackendManager>) {
    if manager.closing.swap(true, Ordering::AcqRel) { return; }
    let ai = app.state::<Arc<ai::AiManager>>().inner().clone();
    ai.shutdown();
    tauri::async_runtime::spawn_blocking(move || {
        match manager.shutdown() {
            Ok(report) => {
                if report.forced {
                    app.dialog().message(report.message).title("后台任务已中断")
                        .kind(MessageDialogKind::Warning).blocking_show();
                }
                app.exit(0);
            }
            Err(error) => {
                manager.closing.store(false, Ordering::Release);
                ai.reopen();
                app.dialog().message(error).title("后台尚未确认退出，请稍后重试关闭")
                    .kind(MessageDialogKind::Error).blocking_show();
            }
        }
    });
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Arc::new(backend::BackendManager::default()))
        .manage(Arc::new(ai::AiManager::default()))
        .manage(Arc::new(ai_settings::SettingsStore::default()))
        .invoke_handler(tauri::generate_handler![
            commands::connect,
            commands::disconnect,
            commands::control_request,
            commands::load_graph,
            commands::save_graph,
            ai_commands::ai_generate,
            ai_commands::ai_summarize,
            ai_commands::ai_cancel_request,
            ai_settings::ai_load_settings,
            ai_settings::ai_save_settings,
            ai_settings::ai_clear_settings
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let manager = window.state::<Arc<backend::BackendManager>>().inner().clone();
                if !manager.shutdown_complete.load(Ordering::Acquire) {
                    api.prevent_close();
                    shutdown_for_close(window.app_handle().clone(), manager);
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("failed to build AudioProcess Tauri prototype")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                let manager = app.state::<Arc<backend::BackendManager>>().inner().clone();
                if !manager.shutdown_complete.load(Ordering::Acquire) {
                    api.prevent_exit();
                    shutdown_for_close(app.clone(), manager);
                }
            }
        });
}
