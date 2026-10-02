//! Human-authored Workflow snapshots use the same interpreter, tools and run records as AI runs.
use crate::agent_runtime::AgentManager;
use crate::agent_tools::ToolContext;
use crate::backend::BackendManager;
use crate::run_records::RunStore;
use crate::tool_workspaces::ToolWorkspaces;
use crate::workflow::{self, Workflow, WorkflowError};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::{Arc, atomic::AtomicBool};
use tauri::Manager;

const MAX_TEXT: usize = 64 * 1024;

fn parse_snapshot(text: &str) -> Result<(Workflow,Value),WorkflowError> {
    if text.len() > MAX_TEXT {
        return Err(WorkflowError { code:"document_limit".into(), message:"Workflow text exceeds 64 KiB".into(),step_path:String::new() });
    }
    let document = crate::graph_files::strict_json(text.as_bytes()).map_err(|message| WorkflowError {
        code:"invalid_json".into(),message,step_path:String::new(),
    })?;
    let program = workflow::validate(&document)?;
    Ok((program,document))
}

fn validate_text(text: &str) -> Value {
    match parse_snapshot(text) {
        Ok((program,_)) => workflow::validation_report(&program),
        Err(error) => json!({"schema_version":1,"valid":false,"error":error}),
    }
}

fn load_text(spaces: &ToolWorkspaces, space: &str, path: &str) -> Result<Value,String> {
    let file = spaces.dispatch("file_read_text",&json!({"space":space,"path":path}))?;
    let text = file.get("content").and_then(Value::as_str).ok_or("File is not UTF-8 text")?;
    Ok(json!({"space":space,"path":path,"text":text}))
}

fn save_text(spaces: &ToolWorkspaces, space: &str, path: &str, text: &str) -> Result<Value,String> {
    parse_snapshot(text).map_err(|error| error.to_string())?;
    spaces.write_new_text(space,path,text)
}

async fn run_snapshot(tools: &mut ToolContext, text: &str, cancel: &AtomicBool) -> Result<Value,String> {
    let (program,document) = parse_snapshot(text).map_err(|error| error.to_string())?;
    let source = json!({"kind":"editor","sha256":format!("{:x}",Sha256::digest(text.as_bytes()))});
    // The inline text is the immutable snapshot; no saved path is implied by its provenance.
    tools.execute_workflow_snapshot(program,document,source,"Workflow editor run",None,Vec::new(),cancel).await
}

fn app_data(app: &tauri::AppHandle) -> Result<std::path::PathBuf,String> {
    app.path().app_data_dir().map_err(|error| format!("App data directory unavailable: {error}"))
}

#[tauri::command]
pub async fn workflow_editor_validate(backend: tauri::State<'_,Arc<BackendManager>>,
    session_id: String, text: String) -> Result<Value,String> {
    // Checking the live session requires no workspace preparation, file writes or backend RPC.
    backend.workspace(&session_id)?;
    Ok(validate_text(&text))
}

#[tauri::command]
pub async fn workflow_editor_load(app: tauri::AppHandle, backend: tauri::State<'_,Arc<BackendManager>>,
    agent: tauri::State<'_,Arc<AgentManager>>, session_id: String, space: String, path: String) -> Result<Value,String> {
    let user_root = backend.workspace(&session_id)?;
    let data = app_data(&app)?;
    let agent = agent.inner().clone();
    tauri::async_runtime::spawn_blocking(move || agent.with_idle(|| {
        load_text(&ToolWorkspaces::new(&user_root,&data)?,&space,&path)
    })).await.map_err(|_| "Workflow load task failed".to_owned())?
}

#[tauri::command]
pub async fn workflow_editor_save(app: tauri::AppHandle, backend: tauri::State<'_,Arc<BackendManager>>,
    agent: tauri::State<'_,Arc<AgentManager>>, session_id: String, space: String, path: String, text: String) -> Result<Value,String> {
    // Reject invalid text before preparing any directories.
    parse_snapshot(&text).map_err(|error| error.to_string())?;
    let user_root = backend.workspace(&session_id)?;
    let data = app_data(&app)?;
    let agent = agent.inner().clone();
    tauri::async_runtime::spawn_blocking(move || agent.with_idle(|| {
        save_text(&ToolWorkspaces::new(&user_root,&data)?,&space,&path,&text)
    })).await.map_err(|_| "Workflow save task failed".to_owned())?
}

#[tauri::command]
pub async fn workflow_editor_run(app: tauri::AppHandle, backend: tauri::State<'_,Arc<BackendManager>>,
    agent: tauri::State<'_,Arc<AgentManager>>, records: tauri::State<'_,Arc<RunStore>>,
    session_id: String, request_id: String, text: String) -> Result<Value,String> {
    let (cancel,_lease) = agent.begin_manual(&request_id)?;
    parse_snapshot(&text).map_err(|error| error.to_string())?;
    let user_root = backend.workspace(&session_id)?;
    let data = app_data(&app)?;
    let spaces = ToolWorkspaces::new(&user_root,&data)?;
    let mut tools = ToolContext::new(spaces,backend.inner().clone(),session_id)
        .with_records(records.inner().clone(),data)
        .with_failure_sink(agent.failure_sink())
        .with_manual_origin();
    // The active lease outlives ToolContext and its confirmed private-backend cleanup.
    run_snapshot(&mut tools,&text,&cancel).await
}

#[cfg(test)]
#[path = "workflow_editor_tests.rs"]
mod tests;
