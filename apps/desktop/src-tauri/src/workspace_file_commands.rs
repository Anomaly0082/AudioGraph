//! Manual UI commands only. Never register these in the AI tool catalog.
use crate::{agent_runtime::AgentManager, backend::BackendManager, commands::ManualRunTracker, tool_workspaces::ToolWorkspaces, workspace_files};
use serde_json::{json,Value};
use std::{collections::VecDeque, path::PathBuf, sync::{Arc,Mutex,atomic::{AtomicU64,Ordering}}, time::{Duration,Instant}};
use tauri::{Emitter,Manager};

struct DropGrant {
    token: String,
    window: String,
    session: String,
    workspace: PathBuf,
    paths: Vec<PathBuf>,
    created: Instant,
}
#[derive(Default)]
pub(crate) struct DropGrants { pending: Mutex<VecDeque<DropGrant>>, next: AtomicU64 }
impl DropGrants {
    // Only called by the native Rust WindowEvent handler, not an IPC command.
    fn capture(&self, window: &str, session: &str, workspace: PathBuf, paths: Vec<PathBuf>) -> Result<Value,String> {
        if paths.is_empty() || paths.len() > 32 || paths.iter().any(|p| !p.is_absolute() || p.as_os_str().len() > 32768) {
            return Err("每次最多拖入 32 个本地文件。".into());
        }
        let serial = self.next.fetch_update(Ordering::Relaxed,Ordering::Relaxed,|n|n.checked_add(1)).map_err(|_| "拖入请求过多，请重启软件")?;
        let token = format!("native-drop-{}-{serial}",std::process::id());
        let names: Vec<_> = paths.iter().map(|path| path.file_name().unwrap_or_default().to_string_lossy().into_owned()).collect();
        let mut pending = self.pending.lock().map_err(|_| "拖入授权不可用")?;
        pending.retain(|grant| grant.created.elapsed() < Duration::from_secs(120));
        while pending.len() >= 8 { pending.pop_front(); }
        pending.push_back(DropGrant { token:token.clone(),window:window.into(),session:session.into(),workspace,paths,created:Instant::now() });
        Ok(json!({"kind":"drop","token":token,"sessionId":session,"names":names}))
    }
    fn take(&self, token: &str, window: &str, session: &str, workspace: &std::path::Path) -> Result<Vec<PathBuf>,String> {
        let mut pending = self.pending.lock().map_err(|_| "拖入授权不可用")?;
        let index = pending.iter().position(|grant| grant.token == token && grant.window == window && grant.session == session && grant.workspace == workspace)
            .ok_or("拖入授权已失效，请在当前工作区重新拖入文件。")?;
        let grant = pending.remove(index).unwrap();
        if grant.created.elapsed() >= Duration::from_secs(120) { return Err("拖入授权已过期，请重新拖入文件。".into()); }
        Ok(grant.paths)
    }
}

pub(crate) fn native_drag(window: &tauri::Window, event: &tauri::DragDropEvent) {
    let payload = match event {
        tauri::DragDropEvent::Enter { .. } => json!({"kind":"enter"}),
        tauri::DragDropEvent::Over { .. } => json!({"kind":"over"}),
        tauri::DragDropEvent::Leave => json!({"kind":"leave"}),
        tauri::DragDropEvent::Drop { paths, .. } => {
            let manager = window.state::<Arc<BackendManager>>();
            match manager.native_drop_context() {
                Some((session,root)) => window.state::<Arc<DropGrants>>().capture(window.label(),&session,root,paths.clone())
                    .unwrap_or_else(|error|json!({"kind":"drop","error":error})),
                None => json!({"kind":"drop","error":"请先打开工作区，再拖入文件。"}),
            }
        },
        _ => return,
    };
    let _ = window.emit_to(window.label(),"workspace-file-drop",payload);
}

#[tauri::command]
pub async fn workspace_files_mutate(app: tauri::AppHandle, window: tauri::Window,
    backend: tauri::State<'_,Arc<BackendManager>>, agent: tauri::State<'_,Arc<AgentManager>>,
    tracker: tauri::State<'_,Arc<ManualRunTracker>>, grants: tauri::State<'_,Arc<DropGrants>>,
    session_id: String, space: String, operation: String, path: String, target: Option<String>, drop_token: Option<String>) -> Result<Value,String> {
    if !matches!(space.as_str(),"user"|"ai") || !matches!(operation.as_str(),"mkdir"|"rename"|"copy"|"move"|"trash"|"import") {
        return Err("不支持的文件操作或工作区。".into());
    }
    let manager=backend.inner().clone();
    let agent=agent.inner().clone();
    let tracker=tracker.inner().clone();
    let grants=grants.inner().clone();
    let label=window.label().to_owned();
    let data=app.path().app_data_dir().map_err(|e|format!("无法定位应用数据目录：{e}"))?;
    tauri::async_runtime::spawn_blocking(move || agent.with_idle(|| {
      if agent.failure_sink().lock().map_err(|_| "AI 后台状态不可用")?.is_some() {
          return Err("AI 后台尚未确认退出，请完成清理或重启后再修改文件。".into());
      }
      manager.with_workspace_operation(&session_id, |root| tracker.with_files_idle(&session_id, || {
        let spaces=ToolWorkspaces::new(root,&data)?;
        let mut result=match operation.as_str() {
            "mkdir" => workspace_files::mkdir(&spaces,&space,&path)?,
            "trash" => workspace_files::recycle(&spaces,&space,&path)?,
            "copy"|"move"|"rename" => {
                let target=target.as_deref().ok_or("请指定目标路径。")?;
                if operation=="rename" && std::path::Path::new(&path).parent()!=std::path::Path::new(target).parent() {
                    return Err("重命名不能更换目录，请使用移动。".into());
                }
                workspace_files::transfer(&spaces,&space,&path,target,operation!="copy")?
            },
            "import" => {
                let token=drop_token.as_deref().ok_or("请重新从系统拖入文件。")?;
                let paths=grants.take(token,&label,&session_id,root)?;
                workspace_files::import_files(&spaces,&space,&path,&paths)?
            },
            _ => unreachable!(),
        };
        result["operation"]=json!(operation);
        result["message"]=json!(match operation.as_str() {
            "mkdir"=>"文件夹已创建。", "copy"=>"复制完成，原文件已保留。",
            "rename"|"move"=>"操作完成。Graph、Workflow 和已打开编辑器中的引用不会自动更新，请按需重新载入。",
            "trash"=>"已移到系统回收站，可在回收站中恢复。",
            _=>"导入完成，外部原文件未修改。",
        });
        Ok(result)
    }))
    })).await.map_err(|e|format!("文件操作任务失败：{e}"))?
}

#[cfg(test)]
mod independent_grant_review_tests {
    use super::*;
    #[test]
    fn guessed_tokens_cannot_supply_files_and_evicted_grants_cannot_be_reused() {
        let store=DropGrants::default();let root=std::env::temp_dir();
        assert!(store.take("native-drop-forged","main","session",&root).is_err());
        assert!(store.capture("main","session",root.clone(),vec![PathBuf::from("relative.wav")]).is_err());
        assert!(store.capture("main","session",root.clone(),vec![]).is_err());
        let reply=store.capture("main","session",root.clone(),vec![root.join("first.wav")]).unwrap();
        let first=reply["token"].as_str().unwrap().to_owned();
        for i in 0..8 {store.capture("main","session",root.clone(),vec![root.join(format!("file-{i}.wav"))]).unwrap();}
        assert!(store.take(&first,"main","session",&root).is_err());
        assert!(store.take("C:/private/secrets.wav","main","session",&root).is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_grants_are_bounded_scoped_expiring_and_single_use() {
        let store=DropGrants::default();
        let root=std::env::temp_dir();
        let paths=vec![root.join("example.wav")];
        let reply=store.capture("main","session-a",root.clone(),paths.clone()).unwrap();
        let token=reply["token"].as_str().unwrap();
        assert!(store.take(token,"other","session-a",&root).is_err());
        assert!(store.take(token,"main","session-b",&root).is_err());
        assert!(store.take(token,"main","session-a",&root.join("elsewhere")).is_err());
        assert_eq!(store.take(token,"main","session-a",&root).unwrap(),paths);
        assert!(store.take(token,"main","session-a",&root).is_err());
        assert!(store.capture("main","session-a",root.clone(),vec![root.join("x");33]).is_err());
        for _ in 0..10 { store.capture("main","session-a",root.clone(),vec![root.join("x")]).unwrap(); }
        assert_eq!(store.pending.lock().unwrap().len(),8);
        let old=store.pending.lock().unwrap().back().unwrap().token.clone();
        store.pending.lock().unwrap().back_mut().unwrap().created=Instant::now()-Duration::from_secs(121);
        assert!(store.take(&old,"main","session-a",&root).is_err());
    }
}
