//! Local read-only file browsing. Bytes never pass through the AI runtime or an asset server.
use crate::backend::BackendManager;
use crate::run_records::opened_file_within_workspace;
use crate::tool_workspaces::ToolWorkspaces;
use serde::Serialize;
use std::fs::{self,File,Metadata};
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration,Instant,UNIX_EPOCH};
use tauri::Manager;

const TEXT_LIMIT: usize = 256 * 1024;
const AUDIO_LIMIT: usize = 32 * 1024 * 1024;
const SCAN_LIMIT: usize = 10_000;
const SCAN_TIME: Duration = Duration::from_secs(2);
const PAGE_LIMIT: usize = 200;

#[derive(Debug,Serialize)]
pub struct BrowserEntry {
    pub name: String,
    pub path: String,
    pub kind: &'static str,
    pub bytes: Option<u64>,
    pub modified_at_ms: Option<u64>,
    #[serde(skip_serializing_if="Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug,Serialize)]
pub struct BrowserList {
    pub space: String,
    pub path: String,
    pub entries: Vec<BrowserEntry>,
    pub offset: usize,
    pub next_offset: Option<usize>,
    pub total: Option<usize>,
    pub partial: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug,Serialize)]
pub struct BrowserText {
    pub space: String,
    pub path: String,
    pub text: String,
    pub bytes: u64,
    pub truncated: bool,
}

fn root<'a>(spaces: &'a ToolWorkspaces, space: &str) -> Result<&'a Path,String> {
    match space { "user" => Ok(&spaces.user_root), "ai" => Ok(&spaces.ai_root),
        _ => Err("Workspace must be user or ai".into()) }
}

fn modified(meta: &Metadata) -> Option<u64> {
    u64::try_from(meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis()).ok()
}

fn list_directory(spaces: &ToolWorkspaces,space: &str,path: &str,offset: usize,limit: usize) -> Result<BrowserList,String> {
    list_directory_bounded(spaces,space,path,offset,limit,SCAN_LIMIT,Instant::now()+SCAN_TIME)
}

fn list_directory_bounded(spaces: &ToolWorkspaces,space: &str,path: &str,offset: usize,limit: usize,
    scan_limit: usize,deadline: Instant) -> Result<BrowserList,String> {
    root(spaces,space)?;
    if limit == 0 || limit > PAGE_LIMIT || offset > SCAN_LIMIT {
        return Err("List limit must be 1 to 200 and offset at most 10000".into());
    }
    let directory = spaces.checked_path(space,path,true)?;
    let mut result = BrowserList { space:space.into(),path:path.into(),entries:vec![],
        offset,next_offset:None,total:Some(0),partial:false,warnings:vec![] };
    if space == "ai" && path.is_empty() && !directory.exists() {
        // A fresh project has no AI scratch space yet. Merely browsing must not create one.
        return Ok(result);
    }
    if !fs::symlink_metadata(&directory).map_err(|error| format!("Cannot inspect directory: {error}"))?.is_dir() {
        return Err("Expected a workspace directory".into());
    }
    let iterator = fs::read_dir(&directory).map_err(|error| format!("Cannot list directory: {error}"))?;
    let mut entries = Vec::new();
    let mut scanned = 0usize;
    let mut failures = 0usize;
    for item in iterator {
        if scanned >= scan_limit || Instant::now() >= deadline {
            result.partial = true;
            result.warnings.push("Directory scan reached its entry or time budget; only the scanned subset is sorted and paginated. Refresh after narrowing the folder.".into());
            break;
        }
        scanned += 1;
        let entry = match item {
            Ok(entry) => entry,
            Err(_) => { failures += 1; result.partial = true; continue; },
        };
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => { failures += 1; result.partial = true; continue; },
        };
        let relative = if path.is_empty() { name.clone() } else { format!("{path}/{name}") };
        let inspected = spaces.checked_path(space,&relative,false).and_then(|target|
            fs::symlink_metadata(target).map_err(|error| format!("Cannot inspect entry: {error}")));
        let item = match inspected {
            Ok(meta) if meta.is_dir() || meta.is_file() => BrowserEntry {
                name,path:relative,kind:if meta.is_dir() { "directory" } else { "file" },
                bytes:if meta.is_file() { Some(meta.len()) } else { None },
                modified_at_ms:modified(&meta),message:None,
            },
            Ok(_) => BrowserEntry { name,path:relative,kind:"unsupported",bytes:None,modified_at_ms:None,
                message:Some("This entry is not an independent regular file or directory".into()) },
            Err(error) => BrowserEntry { name,path:relative,kind:"unsupported",bytes:None,modified_at_ms:None,
                message:Some(error) },
        };
        entries.push(item);
    }
    // Recheck the chain after scanning, rather than returning names from a replaced linked folder.
    spaces.checked_path(space,path,false)?;
    if failures > 0 { result.warnings.push(format!("{failures} unreadable directory entries were skipped")); }
    entries.sort_by(|left,right| {
        let rank = |item: &BrowserEntry| if item.kind == "directory" { 0 } else { 1 };
        rank(left).cmp(&rank(right)).then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.name.cmp(&right.name))
    });
    let count = entries.len();
    result.total = if result.partial { None } else { Some(count) };
    let end = offset.saturating_add(limit).min(count);
    result.next_offset = if end < count { Some(end) } else { None };
    result.entries = entries.into_iter().skip(offset).take(limit).collect();
    Ok(result)
}

fn open_file(spaces: &ToolWorkspaces,space: &str,path: &str) -> Result<(File,Metadata),String> {
    if path.is_empty() { return Err("Select a file, not the workspace root".into()); }
    let target = spaces.checked_path(space,path,false)?;
    let before = fs::symlink_metadata(&target).map_err(|error| format!("Cannot inspect file: {error}"))?;
    if !before.is_file() { return Err("Expected an independent regular file".into()); }
    let file = File::open(&target).map_err(|error| format!("Cannot open file: {error}"))?;
    let opened = file.metadata().map_err(|error| format!("Cannot inspect opened file: {error}"))?;
    if !opened_file_within_workspace(&file,root(spaces,space)?) || !opened.is_file()
        || opened.len() != before.len() || opened.modified().ok() != before.modified().ok() {
        return Err("File changed or escaped its workspace while opening; refresh and try again".into());
    }
    Ok((file,opened))
}

fn read_bytes(spaces: &ToolWorkspaces,space: &str,path: &str,limit: usize,allow_prefix: bool) -> Result<(Vec<u8>,u64,bool),String> {
    let (mut file,before) = open_file(spaces,space,path)?;
    if !allow_prefix && before.len() > limit as u64 { return Err("Audio preview exceeds 32 MiB; choose a smaller file".into()); }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file).take(limit as u64+1).read_to_end(&mut bytes)
        .map_err(|error| format!("Cannot read file: {error}"))?;
    let after = file.metadata().map_err(|error| format!("Cannot inspect file after read: {error}"))?;
    if after.len() != before.len() || after.modified().ok() != before.modified().ok()
        || !opened_file_within_workspace(&file,root(spaces,space)?) {
        return Err("File changed during preview; refresh and try again".into());
    }
    // Check the current path as well; never label an old opened handle as the new linked file.
    spaces.checked_path(space,path,false)?;
    let truncated = bytes.len() > limit;
    if !allow_prefix && truncated { return Err("Audio preview exceeds 32 MiB".into()); }
    if !truncated && bytes.len() as u64 != before.len() {
        return Err("File length changed during preview".into());
    }
    bytes.truncate(limit);
    Ok((bytes,before.len(),truncated))
}

fn preview_text(spaces: &ToolWorkspaces,space: &str,path: &str) -> Result<BrowserText,String> {
    let (mut bytes,size,truncated) = read_bytes(spaces,space,path,TEXT_LIMIT,true)?;
    match std::str::from_utf8(&bytes) {
        Ok(_) => {},
        Err(error) if truncated && error.error_len().is_none() => bytes.truncate(error.valid_up_to()),
        Err(_) => return Err("Text preview supports UTF-8 text only; this file is binary or uses an unsupported encoding".into()),
    }
    let text = String::from_utf8(bytes).map_err(|_| "File is not UTF-8 text")?;
    if text.chars().any(|character| character.is_control() && !matches!(character,'\n' | '\r' | '\t')) {
        return Err("This file contains binary control bytes and cannot be previewed as text".into());
    }
    Ok(BrowserText { space:space.into(),path:path.into(),text,bytes:size,truncated })
}

fn preview_audio(spaces: &ToolWorkspaces,space: &str,path: &str) -> Result<Vec<u8>,String> {
    let extension = Path::new(path).extension().and_then(|value| value.to_str()).unwrap_or("").to_ascii_lowercase();
    if !matches!(extension.as_str(),"wav" | "mp3" | "ogg" | "oga" | "flac" | "m4a" | "aac" | "webm") {
        return Err("Unsupported audio preview extension; use WAV, MP3, OGG, FLAC, M4A, AAC or WebM".into());
    }
    Ok(read_bytes(spaces,space,path,AUDIO_LIMIT,false)?.0)
}

fn data_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf,String> {
    app.path().app_data_dir().map_err(|error| format!("App data directory unavailable: {error}"))
}

#[tauri::command]
pub async fn workspace_browser_list(app: tauri::AppHandle,backend: tauri::State<'_,Arc<BackendManager>>,
    session_id: String,space: String,path: Option<String>,offset: Option<usize>,limit: Option<usize>) -> Result<BrowserList,String> {
    let manager = backend.inner().clone();
    let user_root = manager.workspace(&session_id)?;
    let data = data_path(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        manager.workspace(&session_id)?;
        let spaces = ToolWorkspaces::new_read_only(&user_root,&data)?;
        let result = list_directory(&spaces,&space,path.as_deref().unwrap_or(""),offset.unwrap_or(0),limit.unwrap_or(100))?;
        manager.workspace(&session_id)?;
        Ok(result)
    }).await.map_err(|_| "Workspace listing task failed".to_owned())?
}

#[tauri::command]
pub async fn workspace_browser_text(app: tauri::AppHandle,backend: tauri::State<'_,Arc<BackendManager>>,
    session_id: String,space: String,path: String) -> Result<BrowserText,String> {
    let manager = backend.inner().clone();
    let user_root = manager.workspace(&session_id)?;
    let data = data_path(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        manager.workspace(&session_id)?;
        let result = preview_text(&ToolWorkspaces::new_read_only(&user_root,&data)?,&space,&path)?;
        manager.workspace(&session_id)?;
        Ok(result)
    }).await.map_err(|_| "Text preview task failed".to_owned())?
}

#[tauri::command]
pub async fn workspace_browser_audio(app: tauri::AppHandle,backend: tauri::State<'_,Arc<BackendManager>>,
    session_id: String,space: String,path: String) -> Result<tauri::ipc::Response,String> {
    let manager = backend.inner().clone();
    let user_root = manager.workspace(&session_id)?;
    let data = data_path(&app)?;
    let bytes = tauri::async_runtime::spawn_blocking(move || {
        manager.workspace(&session_id)?;
        let result = preview_audio(&ToolWorkspaces::new_read_only(&user_root,&data)?,&space,&path)?;
        manager.workspace(&session_id)?;
        Ok::<_,String>(result)
    }).await.map_err(|_| "Audio preview task failed".to_owned())??;
    Ok(tauri::ipc::Response::new(bytes))
}

#[cfg(test)]
#[path="workspace_browser_tests.rs"]
mod tests;
