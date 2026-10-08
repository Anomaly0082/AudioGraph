#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod backend;
mod graph_files;
mod editor_file;
mod ai;
mod ai_commands;
mod ai_settings;
mod experiments;
mod ai_experiments;
mod tool_workspaces;
mod agent_tools;
mod agent_runtime;
mod conversation_store;
mod conversation_commands;
mod workflow;
mod workflow_editor;
mod workspace_browser;
mod plugin_snapshot;
mod plugin_manager;
mod run_records;
mod recent_workspaces;
mod session_state;
mod run_history_tools;
#[cfg(test)]
mod run_records_review_tests;
#[cfg(test)]
mod workflow_review_tests;
#[cfg(test)]
#[path = "tool_workspaces_tests.rs"]
mod tool_workspaces_tests;

use std::sync::{Arc, atomic::Ordering};
use tauri::{Manager, Emitter};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind, MessageDialogButtons};

fn request_close(app: tauri::AppHandle) {
    let coordinator=app.state::<Arc<session_state::CloseCoordinator>>().inner().clone();
    let Some(request_id)=coordinator.request() else { return; };
    let _=app.emit("session-state-close-requested",serde_json::json!({"requestId":request_id}));
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        if !coordinator.waiting(&request_id) { return; }
        tauri::async_runtime::spawn_blocking(move || {
            if !coordinator.begin_confirmation(&request_id) { return; }
            let quit=app.dialog().message("尚未确认保存编辑器恢复记录。是否仍然退出？")
                .title("退出前保存未确认").kind(MessageDialogKind::Warning)
                .buttons(MessageDialogButtons::OkCancel).blocking_show();
            if quit && coordinator.claim(&request_id) {
                let manager=app.state::<Arc<backend::BackendManager>>().inner().clone();
                shutdown_for_close(app,manager);
            } else if coordinator.cancel(&request_id) {
                let _=app.emit("session-state-close-cancelled",serde_json::json!({"requestId":request_id}));
            }
        });
    });
}
#[tauri::command]
fn session_state_close_confirmation(app: tauri::AppHandle, request_id: String)->bool {
    app.state::<Arc<session_state::CloseCoordinator>>().begin_confirmation(&request_id)
}
#[tauri::command]
fn session_state_finish_close(app: tauri::AppHandle, request_id: String)->Result<(),String> {
    if !app.state::<Arc<session_state::CloseCoordinator>>().claim(&request_id) { return Err("关闭请求已失效".into()); }
    let manager=app.state::<Arc<backend::BackendManager>>().inner().clone();
    shutdown_for_close(app,manager); Ok(())
}
#[tauri::command]
fn session_state_cancel_close(app: tauri::AppHandle, request_id: String)->Result<(),String> {
    if app.state::<Arc<session_state::CloseCoordinator>>().cancel(&request_id) { Ok(()) } else { Err("关闭请求已失效".into()) }
}

fn shutdown_for_close(app: tauri::AppHandle, manager: Arc<backend::BackendManager>) {
    if manager.closing.swap(true, Ordering::AcqRel) { return; }
    let ai = app.state::<Arc<ai::AiManager>>().inner().clone();
    let agent = app.state::<Arc<agent_runtime::AgentManager>>().inner().clone();
    agent.shutdown();
    ai.shutdown();
    tauri::async_runtime::spawn_blocking(move || {
        let stopped = agent.wait_idle(std::time::Duration::from_secs(12));
        match stopped.and_then(|_| manager.shutdown()) {
            Ok(report) => {
                if report.forced {
                    app.dialog().message(report.message).title("后台任务已中断")
                        .kind(MessageDialogKind::Warning).blocking_show();
                }
                app.exit(0);
            }
            Err(error) => {
                if let Some(request_id)=app.state::<Arc<session_state::CloseCoordinator>>().reset() {
                    let _=app.emit("session-state-close-cancelled",serde_json::json!({"requestId":request_id}));
                }
                manager.closing.store(false, Ordering::Release);
                ai.reopen();
                agent.reopen();
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
        .manage(Arc::new(experiments::ExperimentStore::default()))
        .manage(Arc::new(agent_runtime::AgentManager::default()))
        .manage(Arc::new(conversation_store::ConversationStore::default()))
        .manage(Arc::new(run_records::RunStore::default()))
        .manage(Arc::new(recent_workspaces::RecentWorkspaceStore::default()))
        .manage(Arc::new(session_state::SessionStateStore::default()))
        .manage(Arc::new(session_state::CloseCoordinator::default()))
        .manage(Arc::new(commands::ManualRunTracker::default()))
        .manage(Arc::new(plugin_manager::PluginManager::default()))
        .setup(|app| {
            let manager = app.state::<Arc<backend::BackendManager>>();
            let captured = (|| {
                let directory = app.path().app_config_dir().map_err(|error| error.to_string())?.join("plugins");
                let data = app.path().app_data_dir().map_err(|error| error.to_string())?;
                app.state::<Arc<plugin_manager::PluginManager>>().initialize(&directory,&data)
                    .map_err(|error| format!("插件目录 {}：{error}",directory.display()))
            })();
            // A capture failure is retained by the manager and shown on connection;
            // setup must never silently substitute an empty plugin selection.
            let _ = manager.configure_plugin_snapshot(captured);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::connect,
            commands::disconnect,
            commands::control_request,
            commands::load_graph,
            commands::save_graph,
            ai_commands::ai_generate,
            ai_commands::ai_repair,
            ai_commands::ai_summarize,
            ai_commands::ai_cancel_request,
            ai_settings::ai_load_settings,
            ai_settings::ai_save_settings,
            ai_settings::ai_clear_settings,
            experiments::experiment_create,
            experiments::experiment_list,
            experiments::experiment_load,
            experiments::experiment_save,
            ai_experiments::ai_experiment_candidates,
            agent_runtime::agent_spaces,
            agent_runtime::agent_turn,
            agent_runtime::agent_cancel,
            agent_runtime::agent_reset
            ,conversation_commands::conversation_create
            ,conversation_commands::conversation_list
            ,conversation_commands::conversation_load
            ,run_records::run_records_list
            ,run_records::run_records_load
            ,run_records::run_records_check_files
            ,recent_workspaces::recent_workspaces_list
            ,recent_workspaces::recent_workspaces_remember
            ,recent_workspaces::recent_workspaces_remove
            ,recent_workspaces::recent_workspaces_clear
            ,session_state::session_state_load
            ,session_state::session_state_save
            ,session_state::session_state_clear
            ,session_state_finish_close
            ,session_state_close_confirmation
            ,session_state_cancel_close
            ,workflow_editor::workflow_editor_validate
            ,workflow_editor::workflow_editor_load
            ,workflow_editor::workflow_editor_save
            ,workflow_editor::workflow_editor_run
            ,workspace_browser::workspace_browser_list
            ,workspace_browser::workspace_browser_text
            ,workspace_browser::workspace_browser_audio
            ,plugin_manager::plugin_manager_list
            ,plugin_manager::plugin_manager_install
            ,plugin_manager::plugin_manager_set_enabled
            ,plugin_manager::plugin_manager_uninstall
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let manager = window.state::<Arc<backend::BackendManager>>().inner().clone();
                if !manager.shutdown_complete.load(Ordering::Acquire) {
                    api.prevent_close();
                    request_close(window.app_handle().clone());
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("failed to build AudioProcess Tauri prototype")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
                if code == Some(0) { return; }
                let manager = app.state::<Arc<backend::BackendManager>>().inner().clone();
                if !manager.shutdown_complete.load(Ordering::Acquire) {
                    api.prevent_exit();
                    request_close(app.clone());
                }
            }
        });
}
