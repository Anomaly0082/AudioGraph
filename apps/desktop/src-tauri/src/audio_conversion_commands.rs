//! UI and agent adapters around the same bounded, non-overwriting converter.
use crate::{agent_runtime::AgentManager, audio_conversion::{self, ConversionOptions}, backend::BackendManager, commands::ManualRunTracker, tool_workspaces::ToolWorkspaces};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{sync::{Arc, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use tauri::Manager;

const CONVERSION_TIMEOUT: Duration = Duration::from_secs(120);

fn request_key(window: &str, session: &str, request: &str) -> Result<String, String> {
    if session.is_empty() || session.len() > 256 || session.contains('\0') || request.is_empty()
        || request.len() > 128 || !request.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err("转换请求标识无效".into());
    }
    Ok(format!("audio-convert:{:x}", Sha256::digest(format!("{window}\0{session}\0{request}").as_bytes())))
}

struct CancelManualOnDrop { manager: Arc<AgentManager>, id: String, armed: bool }
impl Drop for CancelManualOnDrop {
    fn drop(&mut self) { if self.armed { self.manager.cancel(&self.id); } }
}

#[tauri::command]
pub async fn workspace_audio_convert(app: tauri::AppHandle, window: tauri::Window,
    backend: tauri::State<'_, Arc<BackendManager>>, agent: tauri::State<'_, Arc<AgentManager>>,
    tracker: tauri::State<'_, Arc<ManualRunTracker>>, session_id: String, space: String,
    path: String, output_path: String, request_id: String, output_format: Option<String>) -> Result<Value, String> {
    if !matches!(space.as_str(), "user" | "ai") { return Err("工作区类型无效".into()); }
    let id = request_key(window.label(), &session_id, &request_id)?;
    let output_format = audio_conversion::OutputFormat::parse(output_format.as_deref().unwrap_or("wav"))?;
    let agent = agent.inner().clone();
    let manager = backend.inner().clone();
    let tracker = tracker.inner().clone();
    let app_data = app.path().app_data_dir().map_err(|error| error.to_string())?;
    let mut guard = CancelManualOnDrop { manager: agent.clone(), id: id.clone(), armed: true };
    let deadline = Instant::now() + CONVERSION_TIMEOUT;
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        // Unlike with_idle, this lease does not hold AgentManager's mutex during
        // conversion. Cancel and application shutdown can set its flag immediately.
        let (cancel, _lease) = agent.begin_manual(&id)?;
        manager.with_workspace_operation(&session_id, |root| tracker.with_files_idle(&session_id, || {
            let spaces = ToolWorkspaces::new(root, &app_data)?;
            audio_conversion::convert(&spaces, &space, &path, &space, &output_path,
                ConversionOptions { output_format, ..Default::default() }, &cancel, deadline)
        }))
    }).await.map_err(|error| format!("转换任务未能完成：{error}"))?;
    guard.armed = false;
    outcome
}

#[tauri::command]
pub fn workspace_audio_convert_cancel(window: tauri::Window, agent: tauri::State<'_, Arc<AgentManager>>,
    session_id: String, request_id: String) -> Result<Value, String> {
    let id = request_key(window.label(), &session_id, &request_id)?;
    // Deliberately do not acquire the workspace lifecycle or file-operation lease.
    // A cancel arriving before the worker starts is remembered by AgentManager.
    Ok(json!({"cancel_requested":agent.cancel(&id)}))
}

struct CancelWorkerOnDrop(Arc<AtomicBool>);
impl Drop for CancelWorkerOnDrop {
    fn drop(&mut self) { self.0.store(true, Ordering::Release); }
}

pub(crate) async fn convert_for_agent(spaces: ToolWorkspaces, source_space: String, source_path: String,
    output_path: String, options: ConversionOptions, cancel: &AtomicBool, workflow_deadline: Option<Instant>) -> Result<Value, String> {
    let deadline = workflow_deadline.map_or_else(|| Instant::now() + CONVERSION_TIMEOUT,
        |limit| limit.min(Instant::now() + CONVERSION_TIMEOUT));
    if cancel.load(Ordering::Acquire) { return Err("Audio conversion cancelled".into()); }
    if Instant::now() >= deadline { return Err("Audio conversion deadline exceeded".into()); }
    let flag = Arc::new(AtomicBool::new(false));
    let _guard = CancelWorkerOnDrop(flag.clone());
    let worker_flag = flag.clone();
    let mut worker = tauri::async_runtime::spawn_blocking(move ||
        audio_conversion::convert(&spaces, &source_space, &source_path, "ai", &output_path, options, &worker_flag, deadline));
    loop {
        if cancel.load(Ordering::Acquire) || Instant::now() >= deadline { flag.store(true, Ordering::Release); }
        tokio::select! {
            result = &mut worker => return result.map_err(|error| format!("Audio conversion worker failed: {error}"))?,
            _ = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_keys_are_scoped_and_pre_cancelled_without_blocking_other_jobs() {
        let id = request_key("main", "session-a", "nonce-1").unwrap();
        assert_ne!(id, request_key("other", "session-a", "nonce-1").unwrap());
        assert_ne!(id, request_key("main", "session-b", "nonce-1").unwrap());
        assert!(request_key("main", "session-a", "bad/path").is_err());
        let manager = AgentManager::default();
        assert!(manager.cancel(&id));
        assert!(manager.begin_manual(&id).is_err());
        let other = request_key("main", "session-a", "nonce-2").unwrap();
        let (flag, lease) = manager.begin_manual(&other).unwrap();
        manager.cancel(&id);
        assert!(!flag.load(Ordering::Acquire));
        manager.cancel(&other);
        assert!(flag.load(Ordering::Acquire));
        drop(lease);
        manager.wait_idle(Duration::from_millis(10)).unwrap();
    }
}
