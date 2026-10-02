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

fn terminal_result(response: &Value, task_id: &str) -> bool {
    let state = response.pointer("/data/state").and_then(Value::as_str);
    response.get("success") == Some(&Value::Bool(true)) && terminal(state)
        && response.pointer("/data/task_id").and_then(Value::as_str) == Some(task_id)
        && (state != Some("succeeded") || response.pointer("/data/result").is_some_and(|result| !result.is_null()))
}

fn interrupt_session(store: &RunStore, tracker: &ManualRunTracker, app_data: &std::path::Path,
    session: &str, workspace: &std::path::Path, reason: &str) -> Vec<String> {
    let spaces = match ToolWorkspaces::new(workspace,app_data) { Ok(spaces) => spaces, Err(error) => return vec![error] };
    let mut tasks = tracker.tasks.lock().unwrap();
    let keys: Vec<_> = tasks.keys().filter(|(id,_)| id == session).cloned().collect();
    let mut warnings = Vec::new();
    for key in keys {
        let run = tasks.get(&key).unwrap();
        if let Err(error) = store.finish(&spaces,app_data,&run.id,"interrupted",None,Some(reason.into()),run.outputs.clone()) {
            warnings.push(format!("中断任务的运行记录保存失败：{error}"));
        } else {
            tasks.remove(&key);
        }
    }
    warnings
}

fn complete_manual(manager: &BackendManager, store: &RunStore, spaces: &ToolWorkspaces,
    app_data: &std::path::Path, tracker: &ManualRunTracker, session: &str, task_id: &str,
    response: &mut Value, result_response: bool) -> bool {
    let key = (session.to_owned(),task_id.to_owned());
    // Do not query a result already persisted (or released). Never hold this lock
    // during an RPC: a failed connection can synchronously interrupt its runs.
    if !tracker.tasks.lock().unwrap().contains_key(&key) { return true; }
    let outcome = if result_response && terminal_result(response,task_id) {
        Ok(response.clone())
    } else {
        match manager.request(session,json!({"op":"tasks.result","task_id":task_id})) {
            Ok(value) if terminal_result(&value,task_id) => Ok(value),
            Ok(_) => Err("尚未取得任务结束后的完整结果，已保留任务，请重试收尾。".to_owned()),
            Err(error) => Err(format!("未能取得任务结束后的完整结果，运行记录仍待保存：{error}")),
        }
    };
    // Another result/status/disconnect callback may have finished this run while
    // the RPC was pending. Serialize the disk write with that second check.
    let mut tasks = tracker.tasks.lock().unwrap();
    let Some(pending) = tasks.get(&key) else { return true; };
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(error) => { warn(response,error); return false; }
    };
    let state = outcome.pointer("/data/state").and_then(Value::as_str).unwrap().to_owned();
    let mut outputs = pending.outputs.clone();
    outputs.extend(crate::agent_tools::typed_result_files(&outcome,"user"));
    let outputs = crate::agent_tools::normalize_file_drafts(spaces,outputs);
    let error = if state == "succeeded" { None } else { Some(outcome.pointer("/data/errors").cloned().unwrap_or(Value::Null).to_string()) };
    if let Err(error) = store.finish(spaces,app_data,&pending.id,&state,Some(outcome),error,outputs) {
        warn(response,format!("运行记录保存失败：{error}"));
        false
    } else {
        tasks.remove(&key);
        true
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
            let mut configuration = request.clone();
            let plugins = crate::agent_tools::graph_plugin_refs(&request["graph"],&catalog);
            if plugins.as_array().is_some_and(|values| !values.is_empty()) { configuration["node_plugins"] = plugins; }
            let started = store.begin(&spaces,&app_data,RunDraft { kind:"graph".into(), origin:"manual".into(),
                parent_id:None,name:"Graph run".into(),configuration,files:inputs });
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
                        if let Some(data) = reply.get_mut("data").and_then(Value::as_object_mut) {
                            data.insert("run_id".into(),json!(record.id));
                        }
                        if let Some(task_id) = task_id {
                            tracker.tasks.lock().unwrap().insert((session_id.to_owned(),task_id.clone()),PendingRun { id:record.id,outputs });
                            if manager.workspace(session_id).is_err() {
                                let warnings = interrupt_session(store,tracker,app_data,session_id,&workspace,"Backend session ended during task start");
                                for warning in warnings { warn(&mut reply,warning); }
                            }
                            if terminal(reply.pointer("/data/state").and_then(Value::as_str)) {
                                complete_manual(&manager,&store,&spaces,&app_data,&tracker,&session_id,&task_id,&mut reply,false);
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
        if op == "tasks.release" {
            let mut recording = json!({});
            if !complete_manual(manager,store,&spaces,app_data,tracker,session_id,&task_id,&mut recording,false) {
                recording["success"] = json!(false);
                recording["errors"] = json!([{"code":"run_record_finalize_failed",
                    "message":"运行结果尚未保存，暂未完成收尾，请重试。"}]);
                return Ok(recording);
            }
            // Persistence must succeed before releasing the backend's only copy.
            // A release reply has no result and must never trigger another fetch.
            return manager.request(session_id,request);
        }
        let mut reply = manager.request(&session_id,request)?;
        if terminal(reply.pointer("/data/state").and_then(Value::as_str)) {
            complete_manual(&manager,&store,&spaces,&app_data,&tracker,&session_id,&task_id,&mut reply,op == "tasks.result");
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
