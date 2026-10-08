//! Only workspace directory names are persisted, independently of connections.
use crate::{backend::BackendManager, run_records};
use serde::{Deserialize, Serialize};
use std::{fs::{self, File}, io::{Read, Write}, path::{Component, Path, PathBuf}, sync::{Arc, Mutex}};
use tauri::Manager;

const MAX_BYTES: usize = 64 * 1024;
const MAX_PATHS: usize = 10;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document { schema_version: u32, paths: Vec<String> }
#[derive(Debug, Serialize)]
pub struct RecentWorkspaces { pub paths: Vec<String> }
#[derive(Default)]
pub struct RecentWorkspaceStore { lock: Mutex<()> }

fn valid_path(value: &str) -> bool {
    !value.is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control)
        && Path::new(value).is_absolute()
        && !Path::new(value).components().any(|part| matches!(part, Component::CurDir | Component::ParentDir))
}
fn key(value: &str) -> String {
    #[cfg(windows)] { value.to_lowercase() }
    #[cfg(not(windows))] { value.to_owned() }
}
fn regular(path: &Path) -> Result<bool, String> {
    run_records::check_chain(path)?;
    match run_records::safe_metadata(path)? {
        None => Ok(false),
        Some(meta) if meta.is_file() && !run_records::is_hardlinked(path, &meta) => Ok(true),
        Some(_) => Err("最近工作区记录不是安全的普通文件，原文件未修改".into()),
    }
}
fn read(path: &Path) -> Result<Vec<String>, String> {
    if !regular(path)? { return Ok(Vec::new()); }
    let mut bytes = Vec::new();
    let file = File::open(path).map_err(|_| "无法读取最近工作区记录")?;
    let parent = path.parent().ok_or("最近工作区记录缺少父目录")?;
    if !run_records::opened_file_within_workspace(&file, parent) || !regular(path)? {
        return Err("最近工作区记录在读取期间发生变化".into());
    }
    file.take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes).map_err(|_| "无法读取最近工作区记录")?;
    if bytes.len() > MAX_BYTES { return Err("最近工作区记录超过64KiB，原文件未修改".into()); }
    let value = crate::graph_files::strict_json(&bytes).map_err(|_| "最近工作区记录损坏，原文件未修改")?;
    let document: Document = serde_json::from_value(value).map_err(|_| "最近工作区记录格式无效，原文件未修改")?;
    if document.schema_version != 1 || document.paths.len() > MAX_PATHS || document.paths.iter().any(|p| !valid_path(p)) {
        return Err("最近工作区记录版本或路径无效，原文件未修改".into());
    }
    let mut seen = std::collections::HashSet::new();
    if document.paths.iter().any(|p| !seen.insert(key(p))) { return Err("最近工作区记录含重复路径，原文件未修改".into()); }
    Ok(document.paths)
}
fn write(path: &Path, paths: &[String]) -> Result<(), String> {
    let existed = regular(path)?;
    let parent = path.parent().ok_or("最近工作区记录缺少父目录")?;
    run_records::check_chain(parent)?;
    fs::create_dir_all(parent).map_err(|_| "无法创建本机配置目录")?;
    run_records::check_chain(parent)?;
    let bytes = serde_json::to_vec(&Document { schema_version: 1, paths: paths.to_vec() }).map_err(|_| "无法编码最近工作区记录")?;
    if bytes.len() > MAX_BYTES { return Err("最近工作区记录超过64KiB，原文件未修改".into()); }
    let (temp, mut file) = run_records::temp_file(parent)?;
    let result = (|| {
        file.write_all(&bytes).and_then(|_| file.sync_all()).map_err(|_| "无法写入最近工作区记录")?;
        drop(file);
        let now_exists = regular(path)?;
        if existed != now_exists { return Err("最近工作区记录在保存期间发生变化".into()); }
        if existed { run_records::replace_file(&temp, path) } else { run_records::create_file(&temp, path) }
    })();
    let _ = fs::remove_file(&temp);
    result
}
impl RecentWorkspaceStore {
    pub(crate) fn list(&self, path: &Path) -> Result<RecentWorkspaces, String> {
        let _guard = self.lock.lock().map_err(|_| "最近工作区记录锁不可用")?;
        read(path).map(|paths| RecentWorkspaces { paths })
    }
    pub(crate) fn remember(&self, path: &Path, workspace: &Path) -> Result<RecentWorkspaces, String> {
        let value = workspace.to_str().ok_or("工作区路径不能编码为文本")?.to_owned();
        if !valid_path(&value) { return Err("工作区路径无效或超过4096字节".into()); }
        let _guard = self.lock.lock().map_err(|_| "最近工作区记录锁不可用")?;
        let mut paths = read(path)?;
        paths.retain(|p| key(p) != key(&value));
        paths.insert(0, value);
        paths.truncate(MAX_PATHS);
        write(path, &paths)?;
        Ok(RecentWorkspaces { paths })
    }
    pub(crate) fn remove(&self, path: &Path, workspace: &str) -> Result<RecentWorkspaces, String> {
        if !valid_path(workspace) { return Err("工作区路径无效".into()); }
        let _guard = self.lock.lock().map_err(|_| "最近工作区记录锁不可用")?;
        let mut paths = read(path)?;
        let previous = paths.len();
        paths.retain(|p| key(p) != key(workspace));
        if previous != paths.len() { write(path, &paths)?; }
        Ok(RecentWorkspaces { paths })
    }
    pub(crate) fn clear(&self, path: &Path) -> Result<RecentWorkspaces, String> {
        let _guard = self.lock.lock().map_err(|_| "最近工作区记录锁不可用")?;
        // Explicit reset may replace corrupt contents, but never links or directories.
        if regular(path)? { write(path, &[])?; }
        Ok(RecentWorkspaces { paths: Vec::new() })
    }
}
fn storage_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path().app_config_dir().map(|p| p.join("recent-workspaces.json")).map_err(|_| "无法确定本机配置目录".into())
}
#[tauri::command]
pub async fn recent_workspaces_list(app: tauri::AppHandle, store: tauri::State<'_, Arc<RecentWorkspaceStore>>) -> Result<RecentWorkspaces, String> {
    let path = storage_path(&app)?; let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.list(&path)).await.map_err(|_| "最近工作区读取任务失败")?
}
#[tauri::command]
pub async fn recent_workspaces_remember(app: tauri::AppHandle, store: tauri::State<'_, Arc<RecentWorkspaceStore>>, manager: tauri::State<'_, Arc<BackendManager>>, session_id: String) -> Result<RecentWorkspaces, String> {
    let workspace = manager.workspace(&session_id)?;
    let path = storage_path(&app)?; let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.remember(&path, &workspace)).await.map_err(|_| "最近工作区保存任务失败")?
}
#[tauri::command]
pub async fn recent_workspaces_remove(app: tauri::AppHandle, store: tauri::State<'_, Arc<RecentWorkspaceStore>>, path: String) -> Result<RecentWorkspaces, String> {
    let storage = storage_path(&app)?; let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.remove(&storage, &path)).await.map_err(|_| "最近工作区移除任务失败")?
}
#[tauri::command]
pub async fn recent_workspaces_clear(app: tauri::AppHandle, store: tauri::State<'_, Arc<RecentWorkspaceStore>>) -> Result<RecentWorkspaces, String> {
    let path = storage_path(&app)?; let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.clear(&path)).await.map_err(|_| "最近工作区清空任务失败")?
}

#[cfg(test)]
#[path = "recent_workspaces_tests.rs"]
mod tests;
