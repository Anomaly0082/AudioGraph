use crate::backend::{BackendManager, ConnectionInfo, DisconnectReport};
use crate::graph_files::{GraphDocument, SavedGraph};
use crate::run_records::{RunDraft,RunFileDraft,RunStore};
use crate::tool_workspaces::ToolWorkspaces;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc,Mutex};
use tauri::Manager;
use tauri::Emitter;

struct PendingRun { id: String, outputs: Vec<RunFileDraft> }
#[derive(Default)]
pub struct ManualRunTracker { tasks: Mutex<HashMap<(String,String),PendingRun>> }

fn warn(reply: &mut Value, message: String) {
    if let Some(object) = reply.as_object_mut() {
        object.entry("record_warnings").or_insert_with(||json!([])).as_array_mut().unwrap().push(json!(message));
    }
}

fn terminal(state: Option<&str>) -> bool { matches!(state,Some("succeeded" | "failed" | "cancelled")) }

fn interrupt_session(store: &RunStore, tracker: &ManualRunTracker, app_data: &std::path::Path,
    session: &str, workspace: &std::path::Path, reason: &str) -> Vec<String> {
    let spaces = match ToolWorkspaces::new(workspace,app_data) { Ok(spaces) => spaces, Err(error) => return vec![error] };
    let pending: Vec<_> = { let mut tasks = tracker.tasks.lock().unwrap();
        let keys: Vec<_> = tasks.keys().filter(|(id,_)| id == session).cloned().collect();
        keys.into_iter().filter_map(|key| tasks.remove(&key)).collect() };
    let mut warnings = Vec::new();
    for run in pending {
        if let Err(error) = store.finish(&spaces,app_data,&run.id,"interrupted",None,Some(reason.into()),run.outputs) {
            warnings.push(format!("Cannot finish interrupted run record: {error}"));
        }
    }
    warnings
}

fn complete_manual(manager: &BackendManager, store: &RunStore, spaces: &ToolWorkspaces,
    app_data: &std::path::Path, tracker: &ManualRunTracker, session: &str, task_id: &str,
    response: &mut Value) {
    let mut missing_result = false;
    let outcome = if response.pointer("/data/result").is_some() {
        response.clone()
    } else {
        match manager.request(session,json!({"op":"tasks.result","task_id":task_id})) {
            Ok(value) if value.get("success") == Some(&Value::Bool(true))
                && terminal(value.pointer("/data/state").and_then(Value::as_str)) => value,
            Ok(_) => {
                warn(response,"Cannot retrieve a terminal tasks.result for run record".into());
                missing_result = true;
                response.clone()
            }
            Err(error) => {
                warn(response,format!("Cannot retrieve final task result for run record: {error}"));
                missing_result = true;
                response.clone()
            }
        }
    };
    let observed_state = outcome.pointer("/data/state").and_then(Value::as_str).unwrap_or("unknown");
    if !terminal(Some(observed_state)) { return; }
    let state = if missing_result { "unknown" } else { observed_state }.to_owned();
    let pending = tracker.tasks.lock().unwrap().get(&(session.to_owned(),task_id.to_owned())).map(|p| (p.id.clone(),p.outputs.clone()));
    let Some(pending) = pending else { return; };
    let mut outputs = pending.1;
    outputs.extend(crate::agent_tools::typed_result_files(&outcome,"user"));
    let outputs = crate::agent_tools::normalize_file_drafts(spaces,outputs);
    let error = if missing_result { Some("Terminal status observed, but tasks.result could not be retrieved".into()) }
        else if state == "succeeded" { None } else { Some(outcome.pointer("/data/errors").cloned().unwrap_or(Value::Null).to_string()) };
    if let Err(error) = store.finish(spaces,app_data,&pending.0,&state,Some(outcome),error,outputs) {
        warn(response,format!("Cannot finish run record: {error}"));
    } else {
        tracker.tasks.lock().unwrap().remove(&(session.to_owned(),task_id.to_owned()));
    }
}

#[tauri::command]
pub async fn connect(app: tauri::AppHandle, state: tauri::State<'_, Arc<BackendManager>>,
                     records: tauri::State<'_,Arc<RunStore>>, tracker: tauri::State<'_,Arc<ManualRunTracker>>,
                     workspace: String, allow_devices: Option<bool>, allow_monitor: Option<bool>) -> Result<ConnectionInfo, String> {
    let manager = state.inner().clone();
    let store = records.inner().clone();
    let tracker = tracker.inner().clone();
    let app_data = app.path().app_data_dir().map_err(|e| format!("App data directory unavailable: {e}"))?;
    let workspace_for_event = std::path::PathBuf::from(&workspace);
    tauri::async_runtime::spawn_blocking(move || manager.connect(workspace,
        allow_devices.unwrap_or(false), allow_monitor.unwrap_or(false), Arc::new(move |mut event| {
            let warnings = interrupt_session(&store,&tracker,&app_data,&event.session_id,&workspace_for_event,&event.message);
            if !warnings.is_empty() { event.message.push_str(&format!("; run record warning: {}",warnings.join("; "))); }
            let _ = app.emit("backend-disconnected", event);
        }))).await.map_err(|e| format!("连接任务失败：{e}"))?
}

#[tauri::command]
pub async fn disconnect(app: tauri::AppHandle, state: tauri::State<'_, Arc<BackendManager>>,
    records: tauri::State<'_,Arc<RunStore>>, tracker: tauri::State<'_,Arc<ManualRunTracker>>,
    session_id: String) -> Result<DisconnectReport, String> {
    let manager = state.inner().clone();
    let store = records.inner().clone();
    let tracker = tracker.inner().clone();
    let app_data = app.path().app_data_dir().map_err(|e| format!("App data directory unavailable: {e}"))?;
    tauri::async_runtime::spawn_blocking(move || {
        let workspace = manager.workspace(&session_id).ok();
        let result = manager.disconnect(&session_id);
        let mut result = result;
        if result.is_ok() {
            if let Some(workspace) = workspace {
                let warnings = interrupt_session(&store,&tracker,&app_data,&session_id,&workspace,"Backend session disconnected");
                if let Ok(report) = &mut result { if !warnings.is_empty() { report.message.push_str(&format!("; run record warning: {}",warnings.join("; "))); } }
            }
        }
        result
    })
        .await.map_err(|e| format!("断开任务失败：{e}"))?
}

#[tauri::command]
pub async fn control_request(app: tauri::AppHandle, state: tauri::State<'_, Arc<BackendManager>>,
    records: tauri::State<'_,Arc<RunStore>>, tracker: tauri::State<'_,Arc<ManualRunTracker>>,
    session_id: String, request: Value) -> Result<Value, String> {
    let manager = state.inner().clone();
    let store = records.inner().clone();
    let tracker = tracker.inner().clone();
    let app_data = app.path().app_data_dir().map_err(|e| format!("App data directory unavailable: {e}"))?;
    tauri::async_runtime::spawn_blocking(move || recorded_control_request(&manager,&store,&tracker,&app_data,&session_id,request))
        .await.map_err(|e| format!("控制请求失败：{e}"))?
}

pub(crate) fn recorded_control_request(manager: &BackendManager, store: &RunStore,
    tracker: &ManualRunTracker, app_data: &std::path::Path, session_id: &str, request: Value) -> Result<Value,String> {
        let op = request.get("op").and_then(Value::as_str).unwrap_or("").to_owned();
        if op != "tasks.start" && !matches!(op.as_str(),"tasks.status" | "tasks.result" | "tasks.cancel" | "tasks.release") {
            return manager.request(&session_id,request);
        }
        let workspace = manager.workspace(&session_id)?;
        let spaces = ToolWorkspaces::new(&workspace,&app_data)?;
        if op == "tasks.start" {
            let catalog = manager.request(&session_id,json!({"op":"nodes.list"}))?;
            if catalog.get("success") != Some(&Value::Bool(true)) {
                return Err("Cannot read node catalog before recording task".into());
            }
            let files = crate::agent_tools::normalize_file_drafts(&spaces,
                crate::agent_tools::graph_file_refs(&request["graph"],&catalog,"user"));
            let (outputs,inputs): (Vec<_>,Vec<_>) = files.into_iter().partition(|file| file.role == "output");
            let started = store.begin(&spaces,&app_data,RunDraft { kind:"graph".into(), origin:"manual".into(),
                parent_id:None,name:"Graph run".into(),configuration:request.clone(),files:inputs });
            let record = started.map_err(|error| format!("Cannot begin run record; task was not started: {error}"))?;
            let mut reply = match manager.request(&session_id,request) {
                Ok(reply) => reply,
                Err(error) => {
                    return Err(match store.finish(&spaces,&app_data,&record.id,"interrupted",None,Some(error.clone()),outputs) {
                        Ok(_) => error, Err(warning) => format!("{error}; run record warning: {warning}") });
                }
            };
            {
                    let task_id = reply.pointer("/data/task_id").and_then(Value::as_str).map(str::to_owned);
                    if reply.get("success") == Some(&Value::Bool(true)) {
                        if let Some(task_id) = task_id {
                            tracker.tasks.lock().unwrap().insert((session_id.to_owned(),task_id.clone()),PendingRun { id:record.id,outputs });
                            if manager.workspace(session_id).is_err() {
                                let warnings = interrupt_session(store,tracker,app_data,session_id,&workspace,"Backend session ended during task start");
                                for warning in warnings { warn(&mut reply,warning); }
                            }
                            if terminal(reply.pointer("/data/state").and_then(Value::as_str)) {
                                complete_manual(&manager,&store,&spaces,&app_data,&tracker,&session_id,&task_id,&mut reply);
                            }
                        } else if let Err(error) = store.finish(&spaces,&app_data,&record.id,"interrupted",Some(reply.clone()),Some("Task start omitted task_id".into()),outputs) {
                            warn(&mut reply,format!("Cannot finish run record: {error}"));
                        }
                    } else {
                        if let Err(error) = store.finish(&spaces,&app_data,&record.id,"failed",Some(reply.clone()),Some("Backend rejected task start".into()),outputs) {
                            warn(&mut reply,format!("Cannot finish run record: {error}"));
                        }
                    }
            }
            return Ok(reply);
        }
        let task_id = request.get("task_id").and_then(Value::as_str).unwrap_or("").to_owned();
        let tracked = tracker.tasks.lock().unwrap().contains_key(&(session_id.to_owned(),task_id.clone()));
        let mut release_warnings = Vec::new();
        if op == "tasks.release" && tracked {
            if let Ok(mut outcome) = manager.request(&session_id,json!({"op":"tasks.result","task_id":task_id})) {
                complete_manual(&manager,&store,&spaces,&app_data,&tracker,&session_id,&task_id,&mut outcome);
                release_warnings = outcome.get("record_warnings").and_then(Value::as_array)
                    .into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
            }
        }
        let mut reply = manager.request(&session_id,request)?;
        for warning in release_warnings { warn(&mut reply,warning); }
        if terminal(reply.pointer("/data/state").and_then(Value::as_str)) || op == "tasks.release" {
            complete_manual(&manager,&store,&spaces,&app_data,&tracker,&session_id,&task_id,&mut reply);
        }
        Ok(reply)
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

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
