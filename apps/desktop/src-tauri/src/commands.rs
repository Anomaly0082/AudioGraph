use crate::backend::{BackendManager, ConnectionInfo, DisconnectReport};
use crate::graph_files::{GraphDocument, SavedGraph};
use serde_json::Value;
use std::sync::Arc;
use tauri::Emitter;

#[tauri::command]
pub async fn connect(app: tauri::AppHandle, state: tauri::State<'_, Arc<BackendManager>>,
                     workspace: String, allow_devices: Option<bool>, allow_monitor: Option<bool>) -> Result<ConnectionInfo, String> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.connect(workspace,
        allow_devices.unwrap_or(false), allow_monitor.unwrap_or(false), Arc::new(move |event| {
            let _ = app.emit("backend-disconnected", event);
        }))).await.map_err(|e| format!("连接任务失败：{e}"))?
}

#[tauri::command]
pub async fn disconnect(state: tauri::State<'_, Arc<BackendManager>>, session_id: String) -> Result<DisconnectReport, String> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.disconnect(&session_id))
        .await.map_err(|e| format!("断开任务失败：{e}"))?
}

#[tauri::command]
pub async fn control_request(state: tauri::State<'_, Arc<BackendManager>>, session_id: String, request: Value) -> Result<Value, String> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.request(&session_id, request))
        .await.map_err(|e| format!("控制请求失败：{e}"))?
}

#[tauri::command]
pub async fn load_graph(state: tauri::State<'_, Arc<BackendManager>>, session_id: String, path: String) -> Result<GraphDocument, String> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let workspace = manager.workspace(&session_id)?;
        crate::graph_files::load_graph_file(&workspace, &path)
    }).await.map_err(|e| format!("载入Graph失败：{e}"))?
}

#[tauri::command]
pub async fn save_graph(state: tauri::State<'_, Arc<BackendManager>>, session_id: String, path: String, graph: Value) -> Result<SavedGraph, String> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let workspace = manager.workspace(&session_id)?;
        crate::graph_files::save_graph_file(&workspace, &path, &graph)
    }).await.map_err(|e| format!("保存Graph失败：{e}"))?
}
