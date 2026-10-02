use crate::agent_runtime::AgentManager;
use crate::agent_tools;
use crate::backend::BackendManager;
use crate::conversation_store::{ConversationDetail, ConversationList, ConversationStore};
use crate::tool_workspaces::ToolWorkspaces;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::Manager;

fn scope(app: &tauri::AppHandle, backend: &BackendManager, session: &str)
    -> Result<(ToolWorkspaces,PathBuf),String> {
    let workspace = backend.workspace(session)?;
    let app_data = app.path().app_data_dir().map_err(|e| format!("App data directory unavailable: {e}"))?;
    Ok((ToolWorkspaces::new(&workspace,&app_data)?,app_data))
}

#[tauri::command]
pub async fn conversation_create(app: tauri::AppHandle, backend: tauri::State<'_,Arc<BackendManager>>,
    store: tauri::State<'_,Arc<ConversationStore>>, agent: tauri::State<'_,Arc<AgentManager>>,
    session_id: String, mode: String, title: Option<String>) -> Result<ConversationDetail,String> {
    if !agent_tools::valid_mode(&mode) { return Err("Invalid conversation mode".into()); }
    let backend = backend.inner().clone(); let store = store.inner().clone(); let agent = agent.inner().clone();
    tauri::async_runtime::spawn_blocking(move || agent.with_idle(|| {
        let (spaces,data) = scope(&app,&backend,&session_id)?;
        store.create(&spaces,&data,&mode,title.as_deref())
    })).await.map_err(|_| "Conversation creation task failed".to_owned())?
}

#[tauri::command]
pub async fn conversation_list(app: tauri::AppHandle, backend: tauri::State<'_,Arc<BackendManager>>,
    store: tauri::State<'_,Arc<ConversationStore>>, session_id: String, mode: Option<String>) -> Result<ConversationList,String> {
    if mode.as_deref().is_some_and(|mode| !agent_tools::valid_mode(mode)) { return Err("Invalid conversation mode".into()); }
    let backend = backend.inner().clone(); let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (spaces,data) = scope(&app,&backend,&session_id)?;
        let mut list = store.list(&spaces,&data)?;
        if let Some(mode) = mode { list.records.retain(|item| item.mode == mode); }
        Ok(list)
    }).await.map_err(|_| "Conversation listing task failed".to_owned())?
}

#[tauri::command]
pub async fn conversation_load(app: tauri::AppHandle, backend: tauri::State<'_,Arc<BackendManager>>,
    store: tauri::State<'_,Arc<ConversationStore>>, agent: tauri::State<'_,Arc<AgentManager>>,
    session_id: String, conversation_id: String, mode: String, before: Option<usize>, limit: Option<usize>)
    -> Result<ConversationDetail,String> {
    if !agent_tools::valid_mode(&mode) { return Err("Invalid conversation mode".into()); }
    let backend = backend.inner().clone(); let store = store.inner().clone(); let agent = agent.inner().clone();
    tauri::async_runtime::spawn_blocking(move || agent.with_idle(|| {
        let (spaces,data) = scope(&app,&backend,&session_id)?;
        let detail = store.load(&spaces,&data,&conversation_id,before,limit)?;
        if detail.mode != mode { return Err("Conversation mode does not match the requested mode".into()); }
        Ok(detail)
    })).await.map_err(|_| "Conversation loading task failed".to_owned())?
}
