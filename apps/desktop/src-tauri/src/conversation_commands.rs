use crate::agent_runtime::AgentManager;
use crate::agent_tools;
use crate::backend::BackendManager;
use crate::conversation_store::{ConversationDeleted, ConversationDetail, ConversationList, ConversationStore};
use crate::commands::ManualRunTracker;
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

fn delete_for_session(backend: &BackendManager, store: &ConversationStore, agent: &AgentManager,
    tracker: &ManualRunTracker, data: &std::path::Path, session: &str, id: &str) -> Result<ConversationDeleted,String> {
    agent.with_idle(|| {
        if agent.failure_sink().lock().map_err(|_| "AI backend state unavailable")?.is_some() {
            return Err("AI backend has not confirmed process exit".into());
        }
        backend.with_workspace_operation(session, |root| tracker.with_files_idle(session, || {
            let spaces = ToolWorkspaces::new_read_only(root,data)?;
            store.delete(&spaces,data,id)
        }))
    })
}

#[tauri::command]
pub async fn conversation_delete(app: tauri::AppHandle, backend: tauri::State<'_,Arc<BackendManager>>,
    store: tauri::State<'_,Arc<ConversationStore>>, agent: tauri::State<'_,Arc<AgentManager>>,
    tracker: tauri::State<'_,Arc<ManualRunTracker>>, session_id: String, conversation_id: String) -> Result<ConversationDeleted,String> {
    let backend = backend.inner().clone(); let store = store.inner().clone();
    let agent = agent.inner().clone(); let tracker = tracker.inner().clone();
    let data = app.path().app_data_dir().map_err(|error| format!("App data directory unavailable: {error}"))?;
    tauri::async_runtime::spawn_blocking(move || delete_for_session(&backend,&store,&agent,&tracker,&data,&session_id,&conversation_id))
        .await.map_err(|_| "Conversation deletion task failed".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delete_rejects_stale_session_and_busy_agent_before_touching_storage() {
        let data = std::env::temp_dir().join(format!("conversation-delete-command-{}",std::process::id()));
        let backend = BackendManager::default(); let store = ConversationStore::default();
        let agent = AgentManager::default(); let tracker = ManualRunTracker::default();
        assert!(delete_for_session(&backend,&store,&agent,&tracker,&data,"stale-session",&"a".repeat(64)).is_err());
        let (_,lease) = agent.begin_manual("active-manual-workflow").unwrap();
        let error = delete_for_session(&backend,&store,&agent,&tracker,&data,"stale-session",&"a".repeat(64)).unwrap_err();
        assert!(error.contains("current agent turn")); drop(lease);
        assert!(!data.exists());
    }
}
