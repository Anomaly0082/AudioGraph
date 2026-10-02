use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration,Instant};

const TEXT_LIMIT: u64 = 64 * 1024;
const COPY_LIMIT: u64 = 256 * 1024 * 1024;
const AI_QUOTA: u64 = 512 * 1024 * 1024;
const AI_DIRECTORY_DEPTH: usize = 3;
const LIST_PAGE_LIMIT: usize = 200;
const LIST_PAGE_BYTES: usize = 16 * 1024;
const LIST_SCAN_LIMIT: usize = 10_000;
const LIST_SCAN_TIME: Duration = Duration::from_secs(2);
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub(crate) struct ToolWorkspaces {
    pub user_root: PathBuf,
    pub ai_root: PathBuf,
}

fn io_error(action: &str, error: std::io::Error) -> String {
    format!("{action}: {error}")
}

fn is_reparse(meta: &Metadata) -> bool {
    if meta.file_type().is_symlink() { return true; }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 { return true; }
    }
    false
}

fn is_hardlinked(path: &Path, meta: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        #[repr(C)]
        struct FileInformation {
            attributes: u32,
            creation_time: [u32; 2],
            access_time: [u32; 2],
            write_time: [u32; 2],
            volume_serial: u32,
            size_high: u32,
            size_low: u32,
            number_of_links: u32,
            file_index_high: u32,
            file_index_low: u32,
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
        return meta.nlink() > 1;
    }
    #[cfg(not(any(windows, unix)))]
    { let _ = (path, meta); true }
}

fn normal_metadata(path: &Path) -> Result<Option<Metadata>, String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if is_reparse(&meta) => Err("Links and reparse points are not allowed".into()),
        Ok(meta) => Ok(Some(meta)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error("Cannot inspect path", error)),
    }
}

fn check_existing_chain(path: &Path) -> Result<(), String> {
    use std::path::Component;
    let mut current = PathBuf::new();
    for part in path.components() {
        current.push(part.as_os_str());
        if matches!(part, Component::Prefix(_)) { continue; }
        if !current.has_root() { continue; }
        if let Some(meta) = normal_metadata(&current)? {
            if !meta.is_dir() && current != path { return Err("Path component is not a directory".into()); }
        }
    }
    Ok(())
}

fn canonical_normal_dir(path: &Path) -> Result<PathBuf, String> {
    check_existing_chain(path)?;
    let canonical = fs::canonicalize(path).map_err(|e| io_error("Cannot locate workspace", e))?;
    check_existing_chain(&canonical)?;
    let meta = normal_metadata(&canonical)?.ok_or("Workspace does not exist")?;
    if !meta.is_dir() { return Err("Workspace is not a directory".into()); }
    Ok(canonical)
}

fn prepare_app_data(path: &Path, user_root: &Path) -> Result<PathBuf, String> {
    use std::path::Component;
    if !path.is_absolute() || path.components().any(|part| matches!(part, Component::ParentDir | Component::CurDir)) {
        return Err("Application data path must be absolute and normalized".into());
    }
    check_existing_chain(path)?;
    let mut missing = Vec::new();
    let mut existing = path;
    while normal_metadata(existing)?.is_none() {
        missing.push(existing.file_name().ok_or("Invalid application data path")?.to_os_string());
        existing = existing.parent().ok_or("Invalid application data path")?;
    }
    let mut current = canonical_normal_dir(existing)?;
    for part in missing.iter().rev() { current.push(part); }
    if contains_path(user_root, &current) || contains_path(&current, user_root) {
        return Err("User workspace overlaps application data".into());
    }
    let mut current = canonical_normal_dir(existing)?;
    for part in missing.iter().rev() {
        current.push(part);
        fs::create_dir(&current).map_err(|e| io_error("Cannot create application data directory", e))?;
        normal_metadata(&current)?.filter(Metadata::is_dir).ok_or("Application data directory changed unexpectedly")?;
    }
    canonical_normal_dir(&current)
}

#[cfg(windows)]
fn path_parts(path: &Path) -> Vec<String> {
    path.components().map(|part| part.as_os_str().to_string_lossy().to_lowercase()).collect()
}
#[cfg(not(windows))]
fn path_parts(path: &Path) -> Vec<String> {
    path.components().map(|part| part.as_os_str().to_string_lossy().into_owned()).collect()
}

fn contains_path(parent: &Path, child: &Path) -> bool {
    let parent = path_parts(parent);
    let child = path_parts(child);
    child.starts_with(&parent)
}

fn relative_parts(path: &str, allow_root: bool) -> Result<Vec<&str>, String> {
    if path.is_empty() && allow_root { return Ok(Vec::new()); }
    if path.is_empty() || path.starts_with('/') || path.contains('\\') || path.contains(':') || path.contains('\0') {
        return Err("Use a nonempty slash relative path".into());
    }
    let parts: Vec<_> = path.split('/').collect();
    for part in &parts {
        if part.is_empty() || *part == "." || *part == ".." || part.ends_with('.') || part.ends_with(' ')
            || part.chars().any(|c| c.is_control() || "<>\"|?*".contains(c)) {
            return Err("Invalid relative path component".into());
        }
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "COM1" | "COM2" | "COM3" | "COM4" | "COM5" | "COM6" | "COM7" | "COM8" | "COM9" | "LPT1" | "LPT2" | "LPT3" | "LPT4" | "LPT5" | "LPT6" | "LPT7" | "LPT8" | "LPT9") {
            return Err("Reserved path component".into());
        }
    }
    Ok(parts)
}

fn exact_args<'a>(args: &'a Value, required: &[&str], optional: &[&str]) -> Result<&'a Map<String, Value>, String> {
    let object = args.as_object().ok_or("Tool arguments must be an object")?;
    for key in object.keys() {
        if !required.contains(&key.as_str()) && !optional.contains(&key.as_str()) { return Err("Unknown tool argument".into()); }
    }
    for key in required {
        if !object.get(*key).is_some_and(Value::is_string) { return Err(format!("Missing or invalid argument: {key}")); }
    }
    for key in optional {
        if object.get(*key).is_some_and(|v| !v.is_string()) { return Err(format!("Invalid argument: {key}")); }
    }
    Ok(object)
}

fn string_arg<'a>(args: &'a Map<String, Value>, name: &str) -> &'a str {
    args.get(name).and_then(Value::as_str).expect("validated string argument")
}

fn listing_args(args: &Value) -> Result<(&str,&str,usize,usize),String> {
    let args = args.as_object().ok_or("Tool arguments must be an object")?;
    if args.keys().any(|key| !matches!(key.as_str(),"space" | "path" | "offset" | "limit")) {
        return Err("Unknown tool argument".into());
    }
    let space = args.get("space").and_then(Value::as_str).ok_or("Missing or invalid argument: space")?;
    let path = match args.get("path") {
        None => "",
        Some(value) => value.as_str().ok_or("Invalid argument: path")?,
    };
    let integer = |key: &str,default: usize,max: usize| -> Result<usize,String> {
        match args.get(key) {
            None => Ok(default),
            Some(value) => value.as_u64().and_then(|value| usize::try_from(value).ok())
                .filter(|value| *value <= max).ok_or_else(|| format!("{key} must be an integer from 0 to {max}")),
        }
    };
    let offset = integer("offset",0,LIST_SCAN_LIMIT)?;
    let limit = integer("limit",100,LIST_PAGE_LIMIT)?;
    if limit == 0 { return Err("limit must be an integer from 1 to 200".into()); }
    Ok((space,path,offset,limit))
}

fn regular_file(path: &Path) -> Result<Metadata, String> {
    let meta = normal_metadata(path)?.ok_or("File does not exist")?;
    if !meta.is_file() || is_hardlinked(path, &meta) { return Err("Expected an independent regular file".into()); }
    Ok(meta)
}

fn create_ai_directory(root: &Path, directory: &Path) -> Result<bool, String> {
    let relative = directory.strip_prefix(root).map_err(|_| "Path escapes AI workspace")?;
    // Check the entire requested depth before creating even the first component.
    if relative.components().count() > AI_DIRECTORY_DEPTH {
        return Err("AI directories support at most 3 levels below the workspace root".into());
    }
    check_existing_chain(root)?;
    if !normal_metadata(root)?.is_some_and(|meta| meta.is_dir()) { return Err("AI workspace is not a directory".into()); }
    let mut current = root.to_path_buf();
    let mut created = false;
    for part in relative.components() {
        current.push(part.as_os_str());
        check_existing_chain(&current)?;
        match normal_metadata(&current)? {
            Some(meta) if meta.is_dir() => {},
            Some(_) => return Err("Parent path is not a directory".into()),
            None => {
                match fs::create_dir(&current) {
                    Ok(()) => { if current == directory { created = true; } },
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {},
                    Err(error) => return Err(io_error("Cannot create AI directory", error)),
                }
                check_existing_chain(&current)?;
                normal_metadata(&current)?.filter(Metadata::is_dir).ok_or("AI directory changed unexpectedly")?;
            }
        }
    }
    Ok(created)
}

fn create_ai_parents(root: &Path, target: &Path) -> Result<(), String> {
    let parent = target.parent().ok_or("File has no parent directory")?;
    create_ai_directory(root, parent).map(|_| ())
}

fn create_user_parent(root: &Path, target: &Path) -> Result<(), String> {
    let parent = target.parent().ok_or("File has no parent")?;
    match normal_metadata(parent)? {
        Some(meta) if meta.is_dir() => {},
        Some(_) => return Err("User file parent must be a directory".into()),
        None if parent.parent() == Some(root) => {
            match fs::create_dir(parent) {
                Ok(()) => {},
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {},
                Err(error) => return Err(io_error("Cannot create user folder", error)),
            }
        },
        None => return Err("Only one new top-level user folder can be created automatically".into()),
    }
    check_existing_chain(parent)?;
    if !normal_metadata(parent)?.is_some_and(|m| m.is_dir()) { return Err("User file parent must be a directory".into()); }
    Ok(())
}

fn temp_file(parent: &Path) -> Result<(PathBuf, File), String> {
    for _ in 0..32 {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let name = format!(".agent-tmp-{}-{serial}", std::process::id());
        let path = parent.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_error("Cannot create temporary file", error)),
        }
    }
    Err("Cannot allocate temporary file".into())
}

#[cfg(windows)]
fn replace_file(temp: &Path, target: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    let from: Vec<_> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<_> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    // Same-directory rename replaces the directory entry without writing through an old hard link.
    let result = unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0x1 | 0x8) };
    if result == 0 { Err(io_error("Cannot replace AI file", std::io::Error::last_os_error())) } else { Ok(()) }
}

#[cfg(not(windows))]
fn replace_file(temp: &Path, target: &Path) -> Result<(), String> {
    fs::rename(temp, target).map_err(|e| io_error("Cannot replace AI file", e))
}

impl ToolWorkspaces {
    // Browsing must not initialize AI scratch directories or write app data.
    pub(crate) fn new_read_only(user_root: &Path, app_data_root: &Path) -> Result<Self,String> {
        use std::path::Component;
        let user_root = canonical_normal_dir(user_root)?;
        if !app_data_root.is_absolute() || app_data_root.components().any(|part|
            matches!(part,Component::ParentDir | Component::CurDir)) {
            return Err("Application data path must be absolute and normalized".into());
        }
        check_existing_chain(app_data_root)?;
        let mut missing = Vec::new();
        let mut existing = app_data_root;
        while normal_metadata(existing)?.is_none() {
            missing.push(existing.file_name().ok_or("Invalid application data path")?.to_os_string());
            existing = existing.parent().ok_or("Invalid application data path")?;
        }
        let mut app_data = canonical_normal_dir(existing)?;
        for part in missing.iter().rev() { app_data.push(part); }
        if contains_path(&user_root,&app_data) || contains_path(&app_data,&user_root) {
            return Err("User workspace overlaps application data".into());
        }
        let hash_input = if cfg!(windows) { user_root.to_string_lossy().to_lowercase() }
            else { user_root.to_string_lossy().into_owned() };
        let ai_root = app_data.join("agent-workspaces")
            .join(format!("{:x}",Sha256::digest(hash_input.as_bytes()))).join("files");
        check_existing_chain(&ai_root)?;
        if normal_metadata(&ai_root)?.is_some_and(|meta| !meta.is_dir()) {
            return Err("AI workspace is not a directory".into());
        }
        Ok(Self { user_root,ai_root })
    }

    pub fn new(user_root: &Path, app_data_root: &Path) -> Result<Self, String> {
        let user_root = canonical_normal_dir(user_root)?;
        let app_data_root = prepare_app_data(app_data_root, &user_root)?;
        if contains_path(&user_root, &app_data_root) || contains_path(&app_data_root, &user_root) {
            return Err("User workspace overlaps application data".into());
        }
        let hash_input = if cfg!(windows) { user_root.to_string_lossy().to_lowercase() } else { user_root.to_string_lossy().into_owned() };
        let digest = Sha256::digest(hash_input.as_bytes());
        let hash = digest.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        let mut current = app_data_root.clone();
        for component in ["agent-workspaces", &hash, "files"] {
            current.push(component);
            match normal_metadata(&current)? {
                Some(meta) if meta.is_dir() => {},
                Some(_) => return Err("AI workspace component is not a normal directory".into()),
                None => fs::create_dir(&current).map_err(|e| io_error("Cannot create AI workspace", e))?,
            }
        }
        let ai_root = canonical_normal_dir(&current)?;
        if contains_path(&user_root, &ai_root) || contains_path(&ai_root, &user_root) {
            return Err("Workspaces overlap".into());
        }
        Ok(Self { user_root, ai_root })
    }

    pub fn checked_path(&self, space: &str, path: &str, allow_missing: bool) -> Result<PathBuf, String> {
        let root = match space { "user" => &self.user_root, "ai" => &self.ai_root, _ => return Err("Invalid workspace".into()) };
        let parts = relative_parts(path, true)?;
        let mut target = root.clone();
        for part in parts { target.push(part); }
        check_existing_chain(&target)?;
        if !contains_path(root, &target) { return Err("Path escapes workspace".into()); }
        match normal_metadata(&target)? {
            Some(meta) if meta.is_file() && is_hardlinked(&target, &meta) => return Err("Hardlinked files are not allowed".into()),
            None if !allow_missing => return Err("Path does not exist".into()),
            _ => {},
        }
        Ok(target)
    }

    fn file_path(&self, space: &str, path: &str, allow_missing: bool) -> Result<PathBuf, String> {
        relative_parts(path, false)?;
        self.checked_path(space, path, allow_missing)
    }

    pub fn check_quota(&self) -> Result<(), String> {
        check_existing_chain(&self.ai_root)?;
        fn walk(dir: &Path, total: &mut u64) -> Result<(), String> {
            for entry in fs::read_dir(dir).map_err(|e| io_error("Cannot inspect AI quota", e))? {
                let entry = entry.map_err(|e| io_error("Cannot inspect AI quota", e))?;
                let path = entry.path();
                let meta = normal_metadata(&path)?.ok_or("AI entry disappeared")?;
                if meta.is_dir() { walk(&path, total)?; }
                else if meta.is_file() && !is_hardlinked(&path, &meta) {
                    *total = total.checked_add(meta.len()).ok_or("AI quota exceeded")?;
                    if *total > AI_QUOTA { return Err("AI quota exceeded".into()); }
                } else { return Err("AI workspace contains an unsupported entry".into()); }
            }
            Ok(())
        }
        walk(&self.ai_root, &mut 0)
    }

    fn quota_size(&self) -> Result<u64, String> {
        check_existing_chain(&self.ai_root)?;
        fn walk(dir: &Path, total: &mut u64) -> Result<(), String> {
            for entry in fs::read_dir(dir).map_err(|e| io_error("Cannot inspect AI quota", e))? {
                let entry = entry.map_err(|e| io_error("Cannot inspect AI quota", e))?;
                let path = entry.path();
                let meta = normal_metadata(&path)?.ok_or("AI entry disappeared")?;
                if meta.is_dir() { walk(&path, total)?; }
                else if meta.is_file() && !is_hardlinked(&path, &meta) { *total = total.checked_add(meta.len()).ok_or("AI quota exceeded")?; }
                else { return Err("AI workspace contains an unsupported entry".into()); }
                if *total > AI_QUOTA { return Err("AI quota exceeded".into()); }
            }
            Ok(())
        }
        let mut size = 0;
        walk(&self.ai_root, &mut size)?;
        Ok(size)
    }

    // Editor save-as has stricter semantics than the AI scratch-file writer: both
    // spaces create a new independent file and preserve every existing target.
    pub(crate) fn write_new_text(&self, space: &str, path: &str, text: &str) -> Result<Value,String> {
        let bytes = text.as_bytes();
        if bytes.len() as u64 > TEXT_LIMIT { return Err("Text exceeds 64 KiB".into()); }
        let target = self.file_path(space,path,true)?;
        if normal_metadata(&target)?.is_some() { return Err("Save as must create a new file; target already exists".into()); }
        if space == "ai" {
            if self.quota_size()?.checked_add(bytes.len() as u64).is_none_or(|sum| sum > AI_QUOTA) {
                return Err("AI quota exceeded".into());
            }
            create_ai_parents(&self.ai_root,&target)?;
        } else { create_user_parent(&self.user_root,&target)?; }
        let target = self.file_path(space,path,true)?;
        let (temp,mut file) = temp_file(target.parent().ok_or("File has no parent")?)?;
        let written = file.write_all(bytes).and_then(|_| file.sync_all());
        drop(file);
        let result = written.map_err(|error| io_error("Cannot write new file",error))
            .and_then(|_| fs::hard_link(&temp,&target).map_err(|error| io_error("Save as must create a new file",error)));
        let removed = fs::remove_file(&temp).map_err(|error| io_error("Cannot remove temporary file",error));
        result?;
        removed?;
        Ok(json!({"space":space,"path":path}))
    }

    pub(crate) fn list_workspace_bounded(&self,space: &str,path: &str,offset: usize,limit: usize,
        scan_limit: usize,deadline: Instant) -> Result<Value,String> {
        if offset > LIST_SCAN_LIMIT || limit == 0 || limit > LIST_PAGE_LIMIT {
            return Err("Listing offset or limit exceeds its bounds".into());
        }
        let dir = self.checked_path(space,path,false)?;
        if !normal_metadata(&dir)?.is_some_and(|meta| meta.is_dir()) { return Err("Expected a directory".into()); }
        // Keep only the sorted prefix needed for this page, even if enumeration is
        // unsorted. The scan itself is separately bounded; no recursive traversal.
        let prefix_limit = offset+limit;
        let mut first: BTreeMap<String,(&str,Option<u64>)> = BTreeMap::new();
        let mut total = 0usize;
        let mut partial = false;
        for entry in fs::read_dir(&dir).map_err(|error| io_error("Cannot list directory",error))? {
            if total >= scan_limit || Instant::now() >= deadline { partial = true; break; }
            let entry = entry.map_err(|error| io_error("Cannot list directory",error))?;
            let meta = normal_metadata(&entry.path())?.ok_or("Entry disappeared")?;
            let kind = if meta.is_dir() { "directory" }
                else if meta.is_file() && !is_hardlinked(&entry.path(),&meta) { "file" }
                else { return Err("Directory contains an unsupported entry".into()); };
            let name = entry.file_name().into_string().map_err(|_| "Directory contains a non-UTF-8 filename")?;
            first.insert(name,(kind,if meta.is_file() { Some(meta.len()) } else { None }));
            if first.len() > prefix_limit { first.pop_last(); }
            total += 1;
        }
        self.checked_path(space,path,false)?;
        let mut entries = Vec::new();
        let mut page_bytes = 2usize; // The serialized array's brackets, plus each row and comma.
        let mut page_byte_limited = false;
        for (name,(kind,bytes)) in first.into_iter().skip(offset).take(limit) {
            let item = json!({"name":name,"kind":kind,"bytes":bytes});
            let encoded = serde_json::to_vec(&item).map_err(|error| format!("Cannot encode directory entry: {error}"))?;
            let row_bytes = encoded.len()+if entries.is_empty() { 0 } else { 1 };
            if page_bytes+row_bytes > LIST_PAGE_BYTES {
                if entries.is_empty() { return Err("A directory entry exceeds the 16 KiB page byte budget".into()); }
                page_byte_limited = true;
                break;
            }
            page_bytes += row_bytes;
            entries.push(item);
        }
        let end = offset.saturating_add(entries.len());
        let next_offset = if end < total { Some(end) } else { None };
        let warnings: Vec<&str> = if partial {
            vec!["Directory scan reached its entry or time budget. Only the scanned subset is sorted and paginated; total is unknown. Do not treat a partial page as a complete directory."]
        } else { Vec::new() };
        Ok(json!({"space":space,"path":path,"entries":entries,"offset":offset,"limit":limit,
            "total":if partial { None } else { Some(total) },"next_offset":next_offset,
            "truncated":partial || next_offset.is_some(),"partial":partial,
            "page_byte_limited":page_byte_limited,"warnings":warnings}))
    }

    pub fn dispatch(&self, name: &str, args: &Value) -> Result<Value, String> {
        match name {
            "workspace_list" => {
                let (space,path,offset,limit) = listing_args(args)?;
                self.list_workspace_bounded(space,path,offset,limit,LIST_SCAN_LIMIT,Instant::now()+LIST_SCAN_TIME)
            }
            "file_read_text" => {
                let args = exact_args(args, &["space", "path"], &[])?;
                let space = string_arg(args, "space");
                let path = string_arg(args, "path");
                let target = self.file_path(space, path, false)?;
                let meta = regular_file(&target)?;
                if meta.len() > TEXT_LIMIT { return Err("Text file exceeds 64 KiB".into()); }
                let mut bytes = Vec::new();
                File::open(&target).map_err(|e| io_error("Cannot read file", e))?.take(TEXT_LIMIT + 1).read_to_end(&mut bytes).map_err(|e| io_error("Cannot read file", e))?;
                if bytes.len() as u64 > TEXT_LIMIT { return Err("Text file exceeds 64 KiB".into()); }
                let content = String::from_utf8(bytes).map_err(|_| "File is not UTF-8 text")?;
                Ok(json!({"space":space,"path":path,"content":content,"bytes":meta.len()}))
            }
            "directory_create" => {
                let args = exact_args(args, &["path"], &[])?;
                let path = string_arg(args, "path");
                let target = self.file_path("ai", path, true)?;
                self.check_quota()?;
                let created = create_ai_directory(&self.ai_root, &target)?;
                Ok(json!({"space":"ai","path":path,"created":created,"max_depth":AI_DIRECTORY_DEPTH}))
            }
            "file_write_text" => {
                let args = exact_args(args, &["path", "content"], &[])?;
                let path = string_arg(args, "path");
                let content = string_arg(args, "content");
                let bytes = content.as_bytes();
                if bytes.len() as u64 > TEXT_LIMIT { return Err("Text exceeds 64 KiB".into()); }
                let target = self.file_path("ai", path, true)?;
                let old_size = match normal_metadata(&target)? {
                    Some(meta) if meta.is_file() && !is_hardlinked(&target, &meta) => meta.len(),
                    Some(_) => return Err("Expected an independent regular file".into()),
                    None => 0,
                };
                let total = self.quota_size()?.saturating_sub(old_size);
                if total.checked_add(bytes.len() as u64).is_none_or(|sum| sum > AI_QUOTA) { return Err("AI quota exceeded".into()); }
                create_ai_parents(&self.ai_root, &target)?;
                let (temp, mut file) = temp_file(target.parent().ok_or("File has no parent")?)?;
                let write_result = file.write_all(bytes).and_then(|_| file.sync_all());
                drop(file);
                if let Err(error) = write_result {
                    let _ = fs::remove_file(&temp);
                    return Err(io_error("Cannot write AI file", error));
                }
                let result = (|| {
                    let existing = normal_metadata(&target)?;
                    if existing.as_ref().is_some_and(|m| !m.is_file() || is_hardlinked(&target, m)) { return Err("Target changed unexpectedly".into()); }
                    if existing.is_some() { replace_file(&temp, &target) }
                    else {
                        fs::hard_link(&temp, &target).map_err(|e| io_error("Cannot create AI file", e))?;
                        Ok(())
                    }
                })();
                let _ = fs::remove_file(&temp);
                result?;
                Ok(json!({"space":"ai","path":path,"bytes":bytes.len()}))
            }
            "file_delete" => {
                let args = exact_args(args, &["path"], &[])?;
                let path = string_arg(args, "path");
                let target = self.file_path("ai", path, false)?;
                let meta = normal_metadata(&target)?.ok_or("Path does not exist")?;
                if meta.is_file() && !is_hardlinked(&target, &meta) { fs::remove_file(&target).map_err(|e| io_error("Cannot delete AI file", e))?; }
                else if meta.is_dir() { fs::remove_dir(&target).map_err(|e| io_error("Directory must be empty", e))?; }
                else { return Err("Unsupported AI entry".into()); }
                Ok(json!({"space":"ai","path":path,"deleted":true}))
            }
            "file_copy_to_ai" => {
                let args = exact_args(args, &["source_space", "source_path", "path"], &[])?;
                let source_space = string_arg(args, "source_space");
                let source_path = string_arg(args, "source_path");
                let path = string_arg(args, "path");
                let source = self.file_path(source_space, source_path, false)?;
                let meta = regular_file(&source)?;
                if meta.len() > COPY_LIMIT { return Err("Copy exceeds 256 MiB".into()); }
                let target = self.file_path("ai", path, true)?;
                if normal_metadata(&target)?.is_some() { return Err("AI target already exists".into()); }
                let used = self.quota_size()?;
                if used.checked_add(meta.len()).is_none_or(|sum| sum > AI_QUOTA) { return Err("AI quota exceeded".into()); }
                let copy_cap = COPY_LIMIT.min(AI_QUOTA - used);
                create_ai_parents(&self.ai_root, &target)?;
                let mut input = File::open(&source).map_err(|e| io_error("Cannot open source", e))?;
                let mut output = OpenOptions::new().write(true).create_new(true).open(&target).map_err(|e| io_error("Cannot create AI file", e))?;
                let copied = std::io::copy(&mut Read::take(&mut input, copy_cap + 1), &mut output);
                let result = match copied {
                    Ok(bytes) if bytes <= copy_cap => output.sync_all().map(|_| bytes).map_err(|e| io_error("Cannot finish AI copy", e)),
                    Ok(_) => Err("Copy exceeds file size or AI quota limit".into()),
                    Err(error) => Err(io_error("Cannot copy file", error)),
                };
                drop(output);
                let bytes = match result { Ok(bytes) => bytes, Err(error) => { let _ = fs::remove_file(&target); return Err(error); } };
                Ok(json!({"space":"ai","path":path,"bytes":bytes}))
            }
            "file_export" => {
                let args = exact_args(args, &["path", "user_path"], &[])?;
                let path = string_arg(args, "path");
                let user_path = string_arg(args, "user_path");
                let source = self.file_path("ai", path, false)?;
                let meta = regular_file(&source)?;
                if meta.len() > COPY_LIMIT { return Err("Export exceeds 256 MiB".into()); }
                // Validate/open the source before creating any user directory.
                let mut input = File::open(&source).map_err(|e| io_error("Cannot open AI file", e))?;
                let target = self.file_path("user", user_path, true)?;
                let parent = target.parent().ok_or("Export has no parent")?;
                match normal_metadata(parent)? {
                    Some(meta) if meta.is_dir() => {},
                    Some(_) => return Err("Export parent must be a directory".into()),
                    None if parent.parent() == Some(self.user_root.as_path()) => {
                        // Only one top-level output folder may be auto-created; no recursive mkdir.
                        match fs::create_dir(parent) {
                            Ok(()) => {},
                            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {},
                            Err(error) => return Err(io_error("Cannot create export folder", error)),
                        }
                    },
                    None => return Err("Only one new top-level export folder can be created automatically".into()),
                }
                // A competing mkdir must not let a link or non-directory bypass validation.
                let target = self.file_path("user", user_path, true)?;
                if !normal_metadata(parent)?.is_some_and(|m| m.is_dir()) { return Err("Export parent must be a directory".into()); }
                let mut output = OpenOptions::new().write(true).create_new(true).open(&target).map_err(|e| io_error("Export must create a new user file", e))?;
                let copied = std::io::copy(&mut Read::take(&mut input, COPY_LIMIT + 1), &mut output);
                let result = match copied {
                    Ok(bytes) if bytes <= COPY_LIMIT => output.sync_all().map(|_| bytes).map_err(|e| io_error("Cannot finish export", e)),
                    Ok(_) => Err("Export exceeds 256 MiB".into()),
                    Err(error) => Err(io_error("Cannot export file", error)),
                };
                drop(output);
                let bytes = match result { Ok(bytes) => bytes, Err(error) => { let _ = fs::remove_file(&target); return Err(error); } };
                Ok(json!({"space":"user","path":user_path,"bytes":bytes}))
            }
            _ => Err("Unknown file tool".into()),
        }
    }
}
