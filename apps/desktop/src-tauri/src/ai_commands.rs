use crate::ai::{AiConfig, AiManager, AiProposal, AiReply};
use crate::backend::BackendManager;
use serde_json::{Value, json};
use std::sync::{Arc, atomic::Ordering};

fn current_session(backend: &BackendManager, session_id: &str) -> Result<(), String> {
    if backend.closing.load(Ordering::Acquire) { return Err("应用正在关闭，不能发送AI请求".into()); }
    // 只核对会话。工作区路径不放入模型请求，图中的路径由用户披露确认。
    backend.workspace(session_id).map(|_| ())
}

async fn backend_read(backend: Arc<BackendManager>, session_id: String, request: Value) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || backend.request(&session_id, request))
        .await.map_err(|_| "后台只读校验任务失败".to_owned())?
}

pub(crate) async fn generate_impl(ai: Arc<AiManager>, backend: Arc<BackendManager>,
    session_id: String, request_id: String, config: AiConfig, prompt: String,
    input_path: Option<String>) -> Result<AiReply, String> {
    let result_id = request_id.clone();
    ai.run(request_id, async move {
        crate::ai::endpoint_url(&config)?;
        current_session(&backend, &session_id)?;
        let inspection = match input_path.map(|path| path.trim().to_owned()).filter(|path| !path.is_empty()) {
            Some(path) => {
                let envelope = backend_read(backend.clone(), session_id.clone(), json!({
                    "op":"audio.inspect", "path":path
                })).await?;
                if envelope.get("success") != Some(&Value::Bool(true)) {
                    let details = envelope.get("errors").map(Value::to_string).unwrap_or_default();
                    return Err(format!("输入音频检查失败，未发送模型请求。{}",
                        details.chars().take(2048).collect::<String>()));
                }
                Some(envelope.get("data").cloned().ok_or("输入音频检查响应缺少data")?)
            }
            None => None,
        };
        let catalog = backend_read(backend.clone(), session_id.clone(), json!({"op":"nodes.list"})).await?;
        let nodes = crate::ai::catalog_nodes(&catalog)?;
        let body = crate::ai::build_generate_body(&config.model, &prompt, &nodes, inspection.as_ref())?;
        current_session(&backend, &session_id)?;
        let response = crate::ai::request_completion(&config, body).await?;
        let mut reply = crate::ai::parse_generation(response, result_id, &nodes)?;
        reply.inspection = inspection;
        if let Some(proposal) = &reply.proposal {
            let validation = backend_read(backend.clone(), session_id.clone(), json!({
                "op":"graph.validate", "mode":proposal.mode, "graph":proposal.graph, "options":proposal.options
            })).await?;
            if validation.get("success") != Some(&Value::Bool(true)) {
                let details = validation.get("errors").map(Value::to_string).unwrap_or_default();
                return Err(format!("AI提案未通过本地校验，没有执行任务。{}", details.chars().take(2048).collect::<String>()));
            }
        }
        current_session(&backend, &session_id)?;
        Ok(reply)
    }).await
}

pub(crate) async fn summarize_impl(ai: Arc<AiManager>, backend: Arc<BackendManager>,
    session_id: String, request_id: String, config: AiConfig, prompt: String,
    proposal: AiProposal, result: Value) -> Result<AiReply, String> {
    let result_id = request_id.clone();
    ai.run(request_id, async move {
        crate::ai::endpoint_url(&config)?;
        current_session(&backend, &session_id)?;
        let body = crate::ai::build_summary_body(&config.model, &prompt, &proposal, &result)?;
        let response = crate::ai::request_completion(&config, body).await?;
        let reply = crate::ai::parse_summary(response, result_id)?;
        current_session(&backend, &session_id)?;
        Ok(reply)
    }).await
}

#[tauri::command]
pub async fn ai_generate(ai: tauri::State<'_, Arc<AiManager>>, backend: tauri::State<'_, Arc<BackendManager>>,
    session_id: String, request_id: String, config: AiConfig, prompt: String,
    input_path: Option<String>) -> Result<AiReply, String> {
    generate_impl(ai.inner().clone(), backend.inner().clone(), session_id, request_id, config, prompt, input_path).await
}

#[tauri::command]
pub async fn ai_summarize(ai: tauri::State<'_, Arc<AiManager>>, backend: tauri::State<'_, Arc<BackendManager>>,
    session_id: String, request_id: String, config: AiConfig, prompt: String,
    proposal: AiProposal, result: Value) -> Result<AiReply, String> {
    summarize_impl(ai.inner().clone(), backend.inner().clone(), session_id, request_id, config, prompt, proposal, result).await
}

#[tauri::command]
pub fn ai_cancel_request(ai: tauri::State<'_, Arc<AiManager>>, request_id: String) -> Value {
    json!({"cancelled": ai.cancel(&request_id)})
}
