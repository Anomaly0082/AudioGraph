use crate::backend::BackendManager;
use crate::tool_workspaces::ToolWorkspaces;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::Manager;

const RECORD_LIMIT: u64 = 6 * 1024 * 1024;
const CONFIGURATION_LIMIT: usize = 4 * 1024 * 1024;
const RESULT_LIMIT: usize = 512 * 1024;
const MAX_FILES: usize = 256;
const LIST_LIMIT: usize = 500;
const SCAN_LIMIT: usize = 10_000;
const HASH_LIMIT: u64 = 256 * 1024 * 1024;
const HASH_BATCH_LIMIT: u64 = 512 * 1024 * 1024;
const HASH_TIME_LIMIT: Duration = Duration::from_secs(5);
const LIST_READ_LIMIT: u64 = 32 * 1024 * 1024;
const LIST_TIME_LIMIT: Duration = Duration::from_secs(2);
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunFileDraft {
    pub space: String,
    pub path: String,
    pub role: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunDraft {
    pub kind: String,
    pub origin: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub configuration: Value,
    pub files: Vec<RunFileDraft>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunFile {
    pub space: String,
    pub path: String,
    pub role: String,
    pub size_bytes: Option<u64>,
    pub sha256: Option<String>,
    pub capture_status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunRecord {
    pub schema_version: u32,
    pub id: String,
    pub kind: String,
    pub origin: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub started_at_ms: u64,
    pub finished_at_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub state: String,
    pub configuration: Value,
    pub result: Option<Value>,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_warning: Option<String>,
    pub files: Vec<RunFile>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunSummary {
    pub id: String,
    pub kind: String,
    pub origin: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub started_at_ms: u64,
    pub finished_at_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub state: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunList {
    pub records: Vec<RunSummary>,
    pub warnings: Vec<String>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct FileCheck {
    pub space: String,
    pub path: String,
    pub role: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Default)]
pub struct RunStore {
    active: Mutex<HashSet<(PathBuf, String)>>,
}

pub(crate) fn time_ms() -> Result<u64, String> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)
        .map_err(|_| "System clock is before Unix epoch")?.as_millis() as u64)
}

pub(crate) fn valid_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn is_link(meta: &Metadata) -> bool {
    if meta.file_type().is_symlink() { return true; }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 { return true; }
    }
    false
}

pub(crate) fn is_hardlinked(path: &Path, meta: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        #[repr(C)]
        struct FileInformation {
            attributes: u32, creation_time: [u32; 2], access_time: [u32; 2],
            write_time: [u32; 2], volume_serial: u32, size_high: u32, size_low: u32,
            number_of_links: u32, file_index_high: u32, file_index_low: u32,
        }
        #[link(name = "Kernel32")]
        unsafe extern "system" {
            fn GetFileInformationByHandle(handle: *mut std::ffi::c_void, info: *mut FileInformation) -> i32;
        }
        if !meta.is_file() { return false; }
        let file = match File::open(path) { Ok(file) => file, Err(_) => return true };
        let mut info = std::mem::MaybeUninit::<FileInformation>::uninit();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } == 0 { return true; }
        return unsafe { info.assume_init().number_of_links > 1 };
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = path;
        meta.nlink() > 1
    }
    #[cfg(not(any(windows, unix)))]
    { let _ = (path, meta); true }
}

pub(crate) fn safe_metadata(path: &Path) -> Result<Option<Metadata>, String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if is_link(&meta) => Err("Links and reparse points are not allowed".into()),
        Ok(meta) => Ok(Some(meta)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Cannot inspect path: {error}")),
    }
}

pub(crate) fn check_chain(path: &Path) -> Result<(), String> {
    use std::path::Component;
    let mut current = PathBuf::new();
    for part in path.components() {
        current.push(part.as_os_str());
        if matches!(part, Component::Prefix(_)) { continue; }
        if let Some(meta) = safe_metadata(&current)? {
            if current != path && !meta.is_dir() { return Err("Path component is not a directory".into()); }
        }
    }
    Ok(())
}

pub(crate) fn ensure_dir(path: &Path) -> Result<(), String> {
    check_chain(path)?;
    match safe_metadata(path)? {
        Some(meta) if meta.is_dir() => Ok(()),
        Some(_) => Err("Storage path is not a directory".into()),
        None => {
            fs::create_dir(path).map_err(|e| format!("Cannot create storage directory: {e}"))?;
            safe_metadata(path)?.filter(Metadata::is_dir)
                .ok_or_else(|| "Storage directory changed unexpectedly".into()).map(|_| ())
        }
    }
}

pub(crate) fn overlaps(a: &Path, b: &Path) -> bool {
    #[cfg(windows)]
    {
        let lower = |p: &Path| p.to_string_lossy().to_lowercase();
        let a = lower(a); let b = lower(b);
        a == b || a.starts_with(&(b.clone() + "\\")) || b.starts_with(&(a + "\\"))
    }
    #[cfg(not(windows))]
    { a.starts_with(b) || b.starts_with(a) }
}

fn storage_dir(spaces: &ToolWorkspaces, app_data: &Path) -> Result<PathBuf, String> {
    use std::path::Component;
    if !app_data.is_absolute() || app_data.components().any(|part| matches!(part, Component::CurDir | Component::ParentDir)) {
        return Err("App data path must be absolute and normalized".into());
    }
    check_chain(app_data)?;
    let app_data = fs::canonicalize(app_data).map_err(|e| format!("Cannot locate app data: {e}"))?;
    check_chain(&app_data)?;
    if overlaps(&app_data, &spaces.user_root) {
        return Err("Run records must be outside user workspace".into());
    }
    let identity = if cfg!(windows) {
        spaces.user_root.to_string_lossy().to_lowercase()
    } else { spaces.user_root.to_string_lossy().into_owned() };
    let workspace_hash = format!("{:x}", Sha256::digest(identity.as_bytes()));
    let mut current = app_data;
    for part in ["run-records", "v1", &workspace_hash, "records"] {
        current.push(part);
        ensure_dir(&current)?;
    }
    if overlaps(&current, &spaces.ai_root) {
        return Err("Run records must be outside AI workspace".into());
    }
    Ok(current)
}

fn record_path(dir: &Path, id: &str) -> Result<PathBuf, String> {
    if !valid_id(id) { return Err("Invalid run ID".into()); }
    Ok(dir.join(format!("{id}.json")))
}

pub(crate) fn temp_file(dir: &Path) -> Result<(PathBuf, File), String> {
    for _ in 0..32 {
        let serial = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(".run-tmp-{}-{serial}", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("Cannot create temporary record: {e}")),
        }
    }
    Err("Cannot allocate temporary record".into())
}

#[cfg(windows)]
pub(crate) fn replace_file(from: &Path, to: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    let from: Vec<_> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<_> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0x1 | 0x8) } == 0 {
        Err(format!("Cannot replace run record: {}", std::io::Error::last_os_error()))
    } else { Ok(()) }
}

#[cfg(windows)]
pub(crate) fn create_file(from: &Path, to: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    let from: Vec<_> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<_> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // No REPLACE_EXISTING flag: creating a record cannot replace another run's ID.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0x8) } == 0 {
        Err(format!("Cannot create run record: {}", std::io::Error::last_os_error()))
    } else { Ok(()) }
}

#[cfg(not(windows))]
pub(crate) fn create_file(from: &Path, to: &Path) -> Result<(), String> {
    fs::hard_link(from, to).map_err(|e| format!("Cannot create run record: {e}"))
}

#[cfg(not(windows))]
pub(crate) fn replace_file(from: &Path, to: &Path) -> Result<(), String> {
    fs::rename(from, to).map_err(|e| format!("Cannot replace run record: {e}"))
}

fn write_record(dir: &Path, record: &RunRecord, replace: bool) -> Result<(), String> {
    let bytes = serde_json::to_vec(record).map_err(|e| format!("Cannot encode record: {e}"))?;
    if bytes.len() as u64 > RECORD_LIMIT { return Err("Run record exceeds 6 MiB".into()); }
    check_chain(dir)?;
    let target = record_path(dir, &record.id)?;
    let (temp, mut output) = temp_file(dir)?;
    let result = (|| {
        output.write_all(&bytes).and_then(|_| output.sync_all())
            .map_err(|e| format!("Cannot write run record: {e}"))?;
        drop(output);
        check_chain(&target)?;
        match safe_metadata(&target)? {
            Some(meta) if replace && meta.is_file() && !is_hardlinked(&target, &meta) => replace_file(&temp, &target),
            None if !replace => create_file(&temp, &target),
            Some(_) if !replace => Err("Run ID already exists".into()),
            _ => Err("Run record target changed unexpectedly".into()),
        }
    })();
    let _ = fs::remove_file(&temp);
    result
}

fn read_record(dir: &Path, id: &str) -> Result<RunRecord, String> {
    let target = record_path(dir, id)?;
    check_chain(&target)?;
    let meta = safe_metadata(&target)?.ok_or("Run record not found")?;
    if !meta.is_file() || is_hardlinked(&target, &meta) { return Err("Run record is not an independent regular file".into()); }
    if meta.len() > RECORD_LIMIT { return Err("Run record exceeds 6 MiB".into()); }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    File::open(&target).map_err(|e| format!("Cannot open run record: {e}"))?
        .take(RECORD_LIMIT + 1).read_to_end(&mut bytes)
        .map_err(|e| format!("Cannot read run record: {e}"))?;
    if bytes.len() as u64 > RECORD_LIMIT { return Err("Run record exceeds 6 MiB".into()); }
    let record: RunRecord = serde_json::from_slice(&bytes).map_err(|e| format!("Invalid run record: {e}"))?;
    if record.schema_version != 1 || record.id != id || !matches!(record.kind.as_str(), "graph" | "workflow")
        || !matches!(record.origin.as_str(), "manual" | "ai") || !valid_state(&record.state)
        || record.files.len() > MAX_FILES {
        return Err("Invalid run record metadata".into());
    }
    Ok(record)
}

fn valid_state(state: &str) -> bool {
    matches!(state, "running" | "succeeded" | "failed" | "cancelled" | "interrupted" | "limited" | "unknown")
}

fn validate_draft(draft: &RunDraft) -> Result<(), String> {
    if !matches!(draft.kind.as_str(), "graph" | "workflow") { return Err("Invalid run kind".into()); }
    if !matches!(draft.origin.as_str(), "manual" | "ai") { return Err("Invalid run origin".into()); }
    if draft.parent_id.as_deref().is_some_and(|id| !valid_id(id)) { return Err("Invalid parent run ID".into()); }
    if draft.name.trim().is_empty() || draft.name.len() > 512 { return Err("Invalid run name".into()); }
    if draft.files.len() > MAX_FILES { return Err("Too many file references".into()); }
    if serde_json::to_vec(&draft.configuration).map_err(|e| format!("Invalid configuration: {e}"))?.len() > CONFIGURATION_LIMIT {
        return Err("Run configuration exceeds 4 MiB".into());
    }
    Ok(())
}

struct HashBudget { remaining: u64, deadline: Instant }

impl HashBudget {
    fn new() -> Self { Self { remaining: HASH_BATCH_LIMIT, deadline: Instant::now() + HASH_TIME_LIMIT } }
}

fn valid_reference_path(path: &str) -> bool {
    if path.is_empty() || path.len() > 1024 || path.starts_with('/') || path.contains('\\')
        || path.contains(':') || path.contains('\0') { return false; }
    path.split('/').all(|part| {
        if part.is_empty() || part == "." || part == ".." || part.ends_with('.') || part.ends_with(' ')
            || part.chars().any(|c| c.is_control() || "<>\"|?*".contains(c)) { return false; }
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "COM1" | "COM2" | "COM3" | "COM4" | "COM5" | "COM6" | "COM7" | "COM8" | "COM9" | "LPT1" | "LPT2" | "LPT3" | "LPT4" | "LPT5" | "LPT6" | "LPT7" | "LPT8" | "LPT9")
    })
}

#[cfg(windows)]
pub(crate) fn opened_file_within_workspace(file: &File, root: &Path) -> bool {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    struct FileInformation {
        attributes: u32, creation_time: [u32; 2], access_time: [u32; 2],
        write_time: [u32; 2], volume_serial: u32, size_high: u32, size_low: u32,
        number_of_links: u32, file_index_high: u32, file_index_low: u32,
    }
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn GetFinalPathNameByHandleW(handle: *mut std::ffi::c_void, path: *mut u16,
            path_length: u32, flags: u32) -> u32;
        fn GetFileInformationByHandle(handle: *mut std::ffi::c_void,
            info: *mut FileInformation) -> i32;
    }
    fn path_key(path: &str) -> String {
        let normal = if let Some(rest) = path.strip_prefix("\\\\?\\UNC\\") {
            format!("\\\\{rest}")
        } else if let Some(rest) = path.strip_prefix("\\\\?\\") {
            rest.to_owned()
        } else { path.to_owned() };
        normal.replace('/', "\\").trim_end_matches('\\').to_lowercase()
    }

    let handle = file.as_raw_handle();
    let mut info = std::mem::MaybeUninit::<FileInformation>::uninit();
    if unsafe { GetFileInformationByHandle(handle, info.as_mut_ptr()) } == 0 { return false; }
    let info = unsafe { info.assume_init() };
    if info.number_of_links != 1 || info.attributes & (0x400 | 0x10) != 0 { return false; }

    let length = unsafe { GetFinalPathNameByHandleW(handle, std::ptr::null_mut(), 0, 0) };
    if length == 0 || length > 32_768 { return false; }
    let mut buffer = vec![0u16; length as usize + 1];
    let read = unsafe { GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), buffer.len() as u32, 0) };
    if read == 0 || read as usize >= buffer.len() { return false; }
    let Ok(actual) = String::from_utf16(&buffer[..read as usize]) else { return false; };
    let actual = path_key(&actual);
    let root = path_key(&root.to_string_lossy());
    actual.starts_with(&(root + "\\"))
}

#[cfg(not(windows))]
pub(crate) fn opened_file_within_workspace(file: &File, root: &Path) -> bool {
    let _ = (file, root);
    true
}

fn capture_file(spaces: &ToolWorkspaces, draft: RunFileDraft, budget: &mut HashBudget) -> Result<RunFile, String> {
    if !matches!(draft.space.as_str(), "user" | "ai") || !matches!(draft.role.as_str(), "input" | "output" | "related")
        || !valid_reference_path(&draft.path) { return Err("Invalid file reference".into()); }
    let mut item = RunFile { space: draft.space, path: draft.path, role: draft.role,
        size_bytes: None, sha256: None, capture_status: "missing".into() };
    let path = match spaces.checked_path(&item.space, &item.path, true) {
        Ok(path) => path,
        Err(_) => { item.capture_status = "unverified".into(); return Ok(item); }
    };
    let meta = match safe_metadata(&path) {
        Ok(Some(meta)) => meta,
        Ok(None) => return Ok(item),
        Err(_) => { item.capture_status = "unverified".into(); return Ok(item); }
    };
    if !meta.is_file() || is_hardlinked(&path, &meta) { item.capture_status = "unverified".into(); return Ok(item); }
    item.size_bytes = Some(meta.len());
    if meta.len() > HASH_LIMIT || meta.len() > budget.remaining || Instant::now() >= budget.deadline {
        item.capture_status = "unverified".into(); return Ok(item);
    }
    let mut file = match File::open(&path) {
        Ok(file) => file,
        Err(_) => { item.capture_status = "unverified".into(); return Ok(item); }
    };
    let root = if item.space == "user" { &spaces.user_root } else { &spaces.ai_root };
    if !opened_file_within_workspace(&file, root)
        || file.metadata().ok().is_none_or(|opened| !opened.is_file() || opened.len() != meta.len()
            || opened.modified().ok() != meta.modified().ok()) {
        item.capture_status = "unverified".into(); return Ok(item);
    }
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let count = match file.read(&mut buffer) {
            Ok(count) => count,
            Err(_) => { item.capture_status = "unverified".into(); return Ok(item); }
        };
        if count == 0 { break; }
        size = match size.checked_add(count as u64) {
            Some(size) => size,
            None => { item.capture_status = "unverified".into(); return Ok(item); }
        };
        if size > HASH_LIMIT || size > budget.remaining || Instant::now() >= budget.deadline {
            item.capture_status = "unverified".into(); return Ok(item);
        }
        digest.update(&buffer[..count]);
    }
    // A concurrent writer must not turn a partial read into a verified snapshot.
    if safe_metadata(&path).ok().flatten().is_none_or(|after| !after.is_file() || after.len() != size
        || after.modified().ok() != meta.modified().ok() || is_hardlinked(&path, &after)) {
        item.capture_status = "unverified".into(); return Ok(item);
    }
    budget.remaining -= size;
    item.size_bytes = Some(size);
    item.sha256 = Some(format!("{:x}", digest.finalize()));
    item.capture_status = "captured".into();
    Ok(item)
}

fn effective_record(mut record: RunRecord, active: &HashSet<(PathBuf, String)>, dir: &Path) -> RunRecord {
    if record.state == "running" && !active.contains(&(dir.to_path_buf(), record.id.clone())) {
        record.state = "interrupted".into();
    }
    record
}

impl RunStore {
    pub fn begin(&self, spaces: &ToolWorkspaces, app_data: &Path, draft: RunDraft) -> Result<RunRecord, String> {
        validate_draft(&draft)?;
        let dir = storage_dir(spaces, app_data)?;
        let mut files = Vec::with_capacity(draft.files.len());
        let mut budget = HashBudget::new();
        for item in draft.files { files.push(capture_file(spaces, item, &mut budget)?); }
        let mut active = self.active.lock().map_err(|_| "Run store lock poisoned")?;
        for _ in 0..32 {
            let started_at_ms = time_ms()?;
            let serial = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let nonce = format!("{}:{}:{}:{}", std::process::id(), started_at_ms,
                SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| "Invalid clock")?.as_nanos(), serial);
            let id = format!("{:x}", Sha256::digest(nonce.as_bytes()));
            let record = RunRecord { schema_version: 1, id: id.clone(), kind: draft.kind.clone(),
                origin: draft.origin.clone(), parent_id: draft.parent_id.clone(), name: draft.name.clone(),
                started_at_ms, finished_at_ms: None, duration_ms: None, state: "running".into(),
                configuration: draft.configuration.clone(), result: None, error: None,
                recording_warning: None, files: files.clone() };
            match write_record(&dir, &record, false) {
                Ok(()) => { active.insert((dir, id)); return Ok(record); }
                Err(error) if error == "Run ID already exists" => continue,
                Err(error) => return Err(error),
            }
        }
        Err("Cannot allocate run ID".into())
    }

    pub fn finish(&self, spaces: &ToolWorkspaces, app_data: &Path, id: &str, state: &str,
        result: Option<Value>, error: Option<String>, additional_files: Vec<RunFileDraft>) -> Result<RunRecord, String> {
        if !matches!(state, "succeeded" | "failed" | "cancelled" | "interrupted" | "limited" | "unknown") {
            return Err("Invalid terminal run state".into());
        }
        if error.as_ref().is_some_and(|message| message.len() > 16 * 1024) { return Err("Run error is too long".into()); }
        if additional_files.len() > MAX_FILES { return Err("Too many file references".into()); }
        let dir = storage_dir(spaces, app_data)?;
        {
            let active = self.active.lock().map_err(|_| "Run store lock poisoned")?;
            if !active.contains(&(dir.clone(), id.to_owned())) { return Err("Run is no longer active".into()); }
        }
        let mut captured = Vec::with_capacity(additional_files.len());
        let mut budget = HashBudget::new();
        for item in additional_files { captured.push(capture_file(spaces, item, &mut budget)?); }
        let mut active = self.active.lock().map_err(|_| "Run store lock poisoned")?;
        if !active.contains(&(dir.clone(), id.to_owned())) { return Err("Run is no longer active".into()); }
        let mut record = read_record(&dir, id)?;
        if record.state != "running" { active.remove(&(dir, id.to_owned())); return Err("Run is already terminal".into()); }
        if record.files.len() + captured.len() > MAX_FILES { return Err("Too many file references".into()); }
        record.files.extend(captured);
        let finished = time_ms()?;
        record.finished_at_ms = Some(finished);
        record.duration_ms = Some(finished.saturating_sub(record.started_at_ms));
        record.state = state.into();
        record.result = result;
        record.error = error;
        if record.result.as_ref().is_some_and(|value|
            serde_json::to_vec(value).is_ok_and(|bytes| bytes.len() > RESULT_LIMIT)) {
            record.result = None;
            record.recording_warning = Some("Run result omitted from history because it exceeds 512 KiB".into());
        }
        if let Err(write_error) = write_record(&dir, &record, true) {
            if record.result.is_none() || !write_error.contains("exceeds 6 MiB") { return Err(write_error); }
            record.result = None;
            record.recording_warning = Some("Run result omitted from history because the record exceeds 6 MiB".into());
            write_record(&dir, &record, true)?;
        }
        active.remove(&(dir, id.to_owned()));
        Ok(record)
    }

    pub fn load(&self, spaces: &ToolWorkspaces, app_data: &Path, id: &str) -> Result<RunRecord, String> {
        let dir = storage_dir(spaces, app_data)?;
        let active = self.active.lock().map_err(|_| "Run store lock poisoned")?.clone();
        Ok(effective_record(read_record(&dir, id)?, &active, &dir))
    }

    pub fn list(&self, spaces: &ToolWorkspaces, app_data: &Path) -> Result<RunList, String> {
        let dir = storage_dir(spaces, app_data)?;
        let active = self.active.lock().map_err(|_| "Run store lock poisoned")?.clone();
        let mut records = Vec::new();
        let mut warnings = Vec::new();
        let mut scanned = 0;
        let mut read_bytes = 0u64;
        let deadline = Instant::now() + LIST_TIME_LIMIT;
        let mut truncated = false;
        for entry in fs::read_dir(&dir).map_err(|e| format!("Cannot list run records: {e}"))? {
            scanned += 1;
            if scanned > SCAN_LIMIT || Instant::now() >= deadline {
                warnings.push("Run history scan limit reached; showing a partial list".into());
                truncated = true;
                break;
            }
            let entry = match entry { Ok(entry) => entry, Err(error) => { warnings.push(format!("Cannot inspect run entry: {error}")); continue; } };
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".json") { continue; }
            let size = match safe_metadata(&entry.path()) {
                Ok(Some(meta)) => meta.len().min(RECORD_LIMIT + 1),
                Ok(None) => { if warnings.len() < 50 { warnings.push(format!("{name}: Record disappeared")); } continue; },
                Err(error) => { if warnings.len() < 50 { warnings.push(format!("{name}: {error}")); } continue; },
            };
            if read_bytes.saturating_add(size) > LIST_READ_LIMIT {
                warnings.push("Run history read limit reached; showing a partial list".into());
                truncated = true;
                break;
            }
            read_bytes += size;
            let id = &name[..name.len() - 5];
            match read_record(&dir, id) {
                Ok(record) => {
                    let r = effective_record(record, &active, &dir);
                    records.push(RunSummary { id: r.id, kind: r.kind, origin: r.origin,
                        parent_id: r.parent_id, name: r.name, started_at_ms: r.started_at_ms,
                        finished_at_ms: r.finished_at_ms, duration_ms: r.duration_ms, state: r.state });
                }
                Err(error) => { if warnings.len() < 50 { warnings.push(format!("{name}: {error}")); } },
            }
        }
        records.sort_by(|a, b| b.started_at_ms.cmp(&a.started_at_ms).then_with(|| b.id.cmp(&a.id)));
        truncated |= records.len() > LIST_LIMIT;
        records.truncate(LIST_LIMIT);
        Ok(RunList { records, warnings, truncated })
    }

    pub fn check_files(&self, spaces: &ToolWorkspaces, app_data: &Path, id: &str) -> Result<Vec<FileCheck>, String> {
        let record = self.load(spaces, app_data, id)?;
        let mut budget = HashBudget::new();
        Ok(record.files.into_iter().map(|stored| {
            let draft = RunFileDraft { space: stored.space.clone(), path: stored.path.clone(), role: stored.role.clone() };
            let (status, message) = match capture_file(spaces, draft, &mut budget) {
                Ok(current) if current.capture_status == "missing" => ("missing", None),
                Ok(_) if stored.sha256.is_none() || stored.size_bytes.is_none() || stored.capture_status != "captured" =>
                    ("unverified", Some("No verified hash was captured when this run executed".into())),
                Ok(current) if current.capture_status != "captured" =>
                    ("unverified", Some("Current file could not be verified".into())),
                Ok(current) if current.size_bytes == stored.size_bytes && current.sha256 == stored.sha256 => ("available", None),
                Ok(_) => ("changed", None),
                Err(error) => ("unverified", Some(error)),
            };
            FileCheck { space: stored.space, path: stored.path, role: stored.role,
                status: status.into(), message }
        }).collect())
    }
}

fn spaces_for_session(backend: &BackendManager, app: &tauri::AppHandle, session_id: &str)
    -> Result<(ToolWorkspaces, PathBuf), String> {
    let workspace = backend.workspace(session_id)?;
    let app_data = app.path().app_data_dir().map_err(|e| format!("App data directory unavailable: {e}"))?;
    let spaces = ToolWorkspaces::new(&workspace, &app_data)?;
    Ok((spaces, app_data))
}

#[tauri::command]
pub async fn run_records_list(app: tauri::AppHandle, backend: tauri::State<'_, Arc<BackendManager>>,
    store: tauri::State<'_, Arc<RunStore>>, session_id: String) -> Result<RunList, String> {
    let backend = backend.inner().clone(); let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (spaces, app_data) = spaces_for_session(&backend, &app, &session_id)?;
        store.list(&spaces, &app_data)
    }).await.map_err(|_| "Run history task failed".to_owned())?
}

#[tauri::command]
pub async fn run_records_load(app: tauri::AppHandle, backend: tauri::State<'_, Arc<BackendManager>>,
    store: tauri::State<'_, Arc<RunStore>>, session_id: String, id: String) -> Result<RunRecord, String> {
    let backend = backend.inner().clone(); let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (spaces, app_data) = spaces_for_session(&backend, &app, &session_id)?;
        store.load(&spaces, &app_data, &id)
    }).await.map_err(|_| "Run history task failed".to_owned())?
}

#[tauri::command]
pub async fn run_records_check_files(app: tauri::AppHandle, backend: tauri::State<'_, Arc<BackendManager>>,
    store: tauri::State<'_, Arc<RunStore>>, session_id: String, id: String) -> Result<Vec<FileCheck>, String> {
    let backend = backend.inner().clone(); let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (spaces, app_data) = spaces_for_session(&backend, &app, &session_id)?;
        store.check_files(&spaces, &app_data, &id)
    }).await.map_err(|_| "Run file check task failed".to_owned())?
}

#[cfg(test)]
#[path = "run_records_tests.rs"]
mod tests;
