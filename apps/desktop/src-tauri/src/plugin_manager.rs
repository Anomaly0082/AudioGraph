//! Human-only management of trusted local plugin folders; never exposes an AI tool.
use crate::backend::{Connection,fixed_sidecar};
use crate::plugin_snapshot::{PluginSnapshot,make_private_directory,read_independent_file,resolved_directory};
use crate::run_records::{check_chain,is_hardlinked,opened_file_within_workspace,replace_file,safe_metadata,temp_file};
use serde::{Deserialize,Serialize};
use serde_json::{Value,json};
use sha2::{Digest,Sha256};
use std::collections::{BTreeMap,BTreeSet};
use std::fs::{self,File,OpenOptions};
use std::io::{Read,Write};
use std::path::{Path,PathBuf};
use std::sync::{Arc,Mutex,atomic::{AtomicU64,Ordering}};
use std::time::{Duration,Instant};

const STATE_LIMIT: usize = 64*1024;
const MANIFEST_LIMIT: usize = 64*1024;
const NODES_LIMIT: usize = 512*1024;
const DLL_LIMIT: u64 = 256*1024*1024;
const ENTRY_LIMIT: usize = 128;
const PACKAGE_TREE_LIMIT: usize = 256;
const PACKAGE_LIMIT: u64 = 1024*1024*1024;
static NEXT: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
thread_local! { static FAIL_STATE_WRITE: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) }; }

#[derive(Debug,Serialize)]
pub struct PluginEntry {
    pub folder: String,
    pub plugin_id: Option<String>,
    pub plugin_version: Option<String>,
    pub node_types: Vec<String>,
    pub enabled: bool,
    pub pending_uninstall: bool,
    pub active: bool,
    pub can_manage: bool,
    pub revision: String,
    pub error: Option<String>,
}
#[derive(Debug,Serialize)]
pub struct PluginView {
    pub directory: String,
    pub restart_required: bool,
    pub entries: Vec<PluginEntry>,
    pub warnings: Vec<String>,
}
#[derive(Clone,Default,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    enabled: bool,
    pending_uninstall: bool,
    #[serde(default)]
    uninstall_identity: Option<String>,
    #[serde(default)]
    error: Option<String>,
}
#[derive(Clone,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct State { schema_version: u32,generation: u64,entries: BTreeMap<String,Policy> }
impl Default for State { fn default() -> Self { Self { schema_version:1,generation:0,entries:BTreeMap::new() } } }

struct Setup {
    directory: PathBuf,
    app_data: PathBuf,
    private: PathBuf,
    active: BTreeSet<String>,
    startup_plan: String,
    warnings: Vec<String>,
}
#[derive(Default)]
struct Inner { setup: Option<Setup>,error: Option<String> }
#[derive(Default)]
pub(crate) struct PluginManager { inner: Mutex<Inner> }
struct Inspected { view: PluginEntry,identity: String,manifest_sha: Option<String> }

fn sha(bytes: &[u8]) -> String { format!("{:x}",Sha256::digest(bytes)) }
fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>,String> { serde_json::to_vec(value).map_err(|error| error.to_string()) }
fn bounded(text: &str,limit: usize) -> String {
    if text.len() <= limit { return text.into(); }
    let mut end = limit;
    while !text.is_char_boundary(end) { end -= 1; }
    format!("{}…",&text[..end])
}
fn folder_name(folder: &str) -> bool {
    if folder.is_empty() || folder.len() > 1024 || folder == "." || folder == ".."
        || folder.ends_with('.') || folder.ends_with(' ')
        || folder.chars().any(|character| character.is_control() || "/\\:<>\"|?*".contains(character)) { return false; }
    let stem = folder.split('.').next().unwrap_or("").to_ascii_uppercase();
    !matches!(stem.as_str(),"CON" | "PRN" | "AUX" | "NUL" | "COM1" | "COM2" | "COM3" | "COM4" | "COM5" | "COM6" | "COM7" | "COM8" | "COM9" | "LPT1" | "LPT2" | "LPT3" | "LPT4" | "LPT5" | "LPT6" | "LPT7" | "LPT8" | "LPT9")
}
fn target(setup: &Setup,folder: &str) -> Result<PathBuf,String> {
    if !folder_name(folder) { return Err("Plugin folder must be one safe direct-child name".into()); }
    check_chain(&setup.directory)?;
    let path = setup.directory.join(folder);
    if path.parent() != Some(setup.directory.as_path()) { return Err("Plugin target escaped the fixed installation directory".into()); }
    check_chain(&path)?;
    Ok(path)
}

struct StateLock { _file: File }
impl StateLock {
    fn acquire(setup: &Setup) -> Result<Self,String> {
        let path = setup.private.join("manager.lock");
        check_chain(&path)?;
        let deadline = Instant::now()+Duration::from_secs(2);
        loop {
            let mut options = OpenOptions::new();
            options.read(true).write(true).create(true);
            #[cfg(windows)] {
                use std::os::windows::fs::OpenOptionsExt;
                options.share_mode(0);
            }
            match options.open(&path) {
                Ok(file) => {
                    if !file.metadata().map_err(|error| error.to_string())?.is_file()
                        || !opened_file_within_workspace(&file,&setup.private) { return Err("Plugin manager lock is not a private independent file".into()); }
                    #[cfg(unix)] {
                        use std::os::fd::AsRawFd;
                        unsafe extern "C" { fn flock(fd: i32,operation: i32) -> i32; }
                        if unsafe { flock(file.as_raw_fd(),2|4) } != 0 {
                            if Instant::now() >= deadline { return Err("Another application instance is updating plugin state".into()); }
                            std::thread::sleep(Duration::from_millis(20));
                            continue;
                        }
                    }
                    return Ok(Self { _file:file });
                },
                Err(error) => {
                    #[cfg(windows)]
                    if error.raw_os_error() == Some(32) && Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(20)); continue;
                    }
                    return Err(format!("Cannot lock private plugin manager state: {error}"));
                },
            }
        }
    }
}

fn load_state(setup: &Setup) -> Result<(State,String),String> {
    let path = setup.private.join("state.json");
    check_chain(&path)?;
    if safe_metadata(&path)?.is_none() { return Ok((State::default(),sha(&[]))); }
    let bytes = read_independent_file(&path,&setup.private,STATE_LIMIT)?;
    let value = crate::graph_files::strict_json(&bytes).map_err(|_| "Plugin manager state is corrupt; it was not reset")?;
    let state: State = serde_json::from_value(value).map_err(|_| "Plugin manager state is corrupt; it was not reset")?;
    if state.schema_version != 1 || state.entries.len() > ENTRY_LIMIT
        || state.entries.iter().any(|(folder,policy)| !folder_name(folder)
            || (policy.pending_uninstall && policy.enabled)
            || policy.error.as_ref().is_some_and(|error| error.len() > 4096)
            || policy.uninstall_identity.as_ref().is_some_and(|identity| identity.len() != 64 || !identity.bytes().all(|byte| byte.is_ascii_hexdigit()))) {
        return Err("Plugin manager state is corrupt or unsupported; it was not reset".into());
    }
    Ok((state,sha(&bytes)))
}
fn save_state(setup: &Setup,state: &mut State) -> Result<(),String> {
    #[cfg(test)]
    if FAIL_STATE_WRITE.with(|counter| match counter.get() {
        Some(1) => { counter.set(None); true },
        Some(left) => { counter.set(Some(left-1)); false },
        None => false,
    }) { return Err("Injected private state write failure".into()); }
    state.generation = state.generation.checked_add(1).ok_or("Plugin state generation exhausted")?;
    let bytes = encode(state)?;
    if bytes.len() > STATE_LIMIT || state.entries.len() > ENTRY_LIMIT { return Err("Plugin manager state exceeds its bounded storage limit".into()); }
    let path = setup.private.join("state.json");
    check_chain(&path)?;
    if let Some(meta) = safe_metadata(&path)? {
        if !meta.is_file() || is_hardlinked(&path,&meta) { return Err("Plugin state is not an independent regular file".into()); }
    }
    let (temporary,mut file) = temp_file(&setup.private)?;
    let written = file.write_all(&bytes).and_then(|_| file.sync_all());
    drop(file);
    if let Err(error) = written { let _ = fs::remove_file(&temporary); return Err(format!("Cannot persist plugin state: {error}")); }
    check_chain(&path)?;
    let result = if safe_metadata(&path)?.is_some() { replace_file(&temporary,&path) }
        else { crate::run_records::create_file(&temporary,&path) };
    let _ = fs::remove_file(&temporary); // Unix create_file links the complete temp before unlinking it here.
    result
}

fn files_metadata(root: &Path) -> Result<Vec<Value>,String> {
    let mut result = Vec::new();
    inventory_directory(root,root,0,&mut result)?;
    result.sort_by(|left,right| left["name"].as_str().cmp(&right["name"].as_str()));
    Ok(result)
}
fn inventory_directory(root: &Path,directory: &Path,depth: usize,result: &mut Vec<Value>) -> Result<(),String> {
    if depth > 4 { return Err("Plugin tree is too deep".into()); }
    check_chain(directory)?;
    for item in fs::read_dir(directory).map_err(|error| error.to_string())? {
        if result.len() >= PACKAGE_TREE_LIMIT { return Err("Plugin folder contains too many entries".into()); }
        let item = item.map_err(|error| error.to_string())?;
        let path = item.path();
        let meta = safe_metadata(&path)?.ok_or("Plugin member disappeared")?;
        if meta.is_file() && is_hardlinked(&path,&meta) { return Err("Hardlinked plugin members are not manageable".into()); }
        if !meta.is_file() && !meta.is_dir() { return Err("Plugin member is not a regular file or directory".into()); }
        let name = path.strip_prefix(root).map_err(|_| "Plugin member escaped root")?.to_str().ok_or("Plugin member name is not Unicode")?.replace('\\',"/");
        result.push(json!({"name":name,"bytes":meta.len(),"directory":meta.is_dir(),
            "modified":meta.modified().ok().and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok()).map(|duration| duration.as_nanos().to_string())}));
        if meta.is_dir() { inventory_directory(root,&path,depth+1,result)?; }
    }
    Ok(())
}
fn inspect(setup: &Setup,folder: &str,state: &State,state_sha: &str) -> Inspected {
    let policy = state.entries.get(folder);
    let mut view = PluginEntry { folder:folder.into(),plugin_id:None,plugin_version:None,node_types:vec![],
        enabled:policy.map_or(true,|policy| policy.enabled),pending_uninstall:policy.is_some_and(|policy| policy.pending_uninstall),
        active:setup.active.contains(folder),can_manage:false,revision:String::new(),error:policy.and_then(|policy| policy.error.clone()) };
    let mut manifest_sha = None;
    let mut members = Vec::new();
    let checked = (|| -> Result<(),String> {
        let root = target(setup,folder)?;
        if !safe_metadata(&root)?.is_some_and(|meta| meta.is_dir()) { return Err("Plugin target is not a normal directory".into()); }
        members = files_metadata(&root)?;
        view.can_manage = true;
        let bytes = read_independent_file(&root.join("manifest.json"),&root,MANIFEST_LIMIT)?;
        manifest_sha = Some(sha(&bytes));
        let manifest = crate::graph_files::strict_json(&bytes).map_err(|_| "Plugin manifest is invalid JSON")?;
        view.plugin_id = manifest.get("plugin_id").and_then(Value::as_str).filter(|value| value.len() <= 128).map(str::to_owned);
        view.plugin_version = manifest.get("plugin_version").and_then(Value::as_str).filter(|value| value.len() <= 128).map(str::to_owned);
        let nodes_bytes = read_independent_file(&root.join("nodes.json"),&root,NODES_LIMIT)?;
        let catalog = crate::graph_files::strict_json(&nodes_bytes).map_err(|_| "Static node catalog is invalid JSON")?;
        let nodes = catalog.get("nodes").and_then(Value::as_array).ok_or("Static node catalog is missing a nodes array")?;
        view.node_types = nodes.iter().take(64)
            .filter_map(|node| node.get("typeId").and_then(Value::as_str).filter(|value| !value.is_empty() && value.len() <= 128).map(str::to_owned)).collect();
        if nodes.len() > 64 || view.node_types.len() != nodes.len() { return Err("Static node catalog exceeds 64 nodes or has invalid IDs; the displayed node list is incomplete".into()); }
        if view.plugin_id.is_none() || view.plugin_version.is_none() { return Err("Plugin manifest lacks a valid identity".into()); }
        Ok(())
    })();
    if let Err(error) = checked { view.error = Some(error); }
    let identity = sha(&encode(&json!({"folder":folder,"manifest":manifest_sha,"members":members})).unwrap_or_default());
    view.revision = sha(format!("{state_sha}:{identity}").as_bytes());
    Inspected { view,identity,manifest_sha }
}
fn installed(setup: &Setup,state: &State,state_sha: &str) -> Result<Vec<Inspected>,String> {
    check_chain(&setup.directory)?;
    let mut folders = Vec::new();
    for (index,item) in fs::read_dir(&setup.directory).map_err(|error| format!("Cannot list plugin directory: {error}"))?.enumerate() {
        if index >= ENTRY_LIMIT { return Err("Plugin directory exceeds 128 entries".into()); }
        let item = item.map_err(|error| error.to_string())?;
        let folder = item.file_name().into_string().map_err(|_| "Plugin folder is not Unicode")?;
        let metadata = fs::symlink_metadata(item.path()).map_err(|error| error.to_string())?;
        if metadata.is_dir() || metadata.file_type().is_symlink() { folders.push(folder); }
    }
    folders.sort();
    Ok(folders.into_iter().map(|folder| inspect(setup,&folder,state,state_sha)).collect())
}
fn plan(entries: &[Inspected]) -> String {
    let selected: BTreeMap<&str,&Option<String>> = entries.iter().filter(|item| item.view.enabled && !item.view.pending_uninstall)
        .map(|item| (item.view.folder.as_str(),&item.manifest_sha)).collect();
    sha(&encode(&selected).unwrap_or_default())
}
fn view(setup: &Setup,state: &State,state_sha: &str) -> Result<PluginView,String> {
    let entries = installed(setup,state,state_sha)?;
    Ok(PluginView { directory:setup.directory.to_string_lossy().into_owned(),restart_required:plan(&entries) != setup.startup_plan
        || state.entries.values().any(|policy| policy.pending_uninstall),
        entries:entries.into_iter().map(|item| item.view).collect(),warnings:setup.warnings.clone() })
}

fn move_new(from: &Path,to: &Path) -> Result<(),String> {
    check_chain(from)?; check_chain(to)?;
    if safe_metadata(to)?.is_some() { return Err("Destination already exists; it will not be overwritten".into()); }
    #[cfg(windows)] {
        use std::os::windows::ffi::OsStrExt;
        unsafe extern "system" { fn MoveFileExW(from: *const u16,to: *const u16,flags: u32) -> i32; }
        let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe { MoveFileExW(from.as_ptr(),to.as_ptr(),0x8) } == 0 { return Err(format!("Cannot move plugin folder: {}",std::io::Error::last_os_error())); }
        Ok(())
    }
    #[cfg(target_os="linux")] {
        use std::os::unix::ffi::OsStrExt;
        unsafe extern "C" { fn renameat2(oldfd: i32,old: *const i8,newfd: i32,new: *const i8,flags: u32) -> i32; }
        let from = std::ffi::CString::new(from.as_os_str().as_bytes()).map_err(|_| "Invalid source path")?;
        let to = std::ffi::CString::new(to.as_os_str().as_bytes()).map_err(|_| "Invalid destination path")?;
        if unsafe { renameat2(-100,from.as_ptr(),-100,to.as_ptr(),1) } != 0 { return Err(format!("Cannot move new plugin folder: {}",std::io::Error::last_os_error())); }
        Ok(())
    }
    #[cfg(not(any(windows,target_os="linux")))] { let _ = (from,to); Err("Atomic plugin folder publication is unsupported on this platform".into()) }
}
fn unique_directory(parent: &Path,prefix: &str) -> Result<PathBuf,String> {
    make_private_directory(parent)?;
    for _ in 0..32 {
        let name = format!("{prefix}-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed));
        let path = parent.join(name);
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Cannot reserve private plugin operation folder: {error}")),
        }
    }
    Err("Cannot reserve a unique plugin operation folder".into())
}
fn move_to_backup(setup: &Setup,source: &Path,container: &str) -> Result<PathBuf,String> {
    let reserved = unique_directory(&setup.private.join(container),"backup")?;
    let destination = reserved.join("package");
    move_new(source,&destination)?;
    Ok(destination)
}
fn validate_tree(root: &Path,depth: usize,count: &mut usize) -> Result<(),String> {
    if depth > 4 { return Err("Plugin tree is too deep to move safely".into()); }
    check_chain(root)?;
    for item in fs::read_dir(root).map_err(|error| error.to_string())? {
        *count += 1;
        if *count > PACKAGE_TREE_LIMIT { return Err("Plugin tree has too many entries to move safely".into()); }
        let item = item.map_err(|error| error.to_string())?;
        let meta = safe_metadata(&item.path())?.ok_or("Plugin member disappeared")?;
        if meta.is_dir() { validate_tree(&item.path(),depth+1,count)?; }
        else if !meta.is_file() || is_hardlinked(&item.path(),&meta) { return Err("Plugin uninstall refuses linked or unsupported members".into()); }
    }
    Ok(())
}

// Probe DELETE access before moving. An existing production READ-only DLL pin
// rejects these probes. Our handles must then close before the folder rename:
// Windows rejects parent-folder moves with our own open member handles as well.
// MoveFileEx itself also rejects a production pin acquired before the rename.
fn move_access(root: &Path) -> Result<Vec<File>,String> {
    #[cfg(windows)] {
        use std::os::windows::fs::OpenOptionsExt;
        fn collect(directory: &Path,root: &Path,handles: &mut Vec<File>) -> Result<(),String> {
            for item in fs::read_dir(directory).map_err(|error| error.to_string())? {
                let item = item.map_err(|error| error.to_string())?;
                let path = item.path();
                let meta = safe_metadata(&path)?.ok_or("Plugin member disappeared")?;
                if meta.is_dir() { collect(&path,root,handles)?; }
                else {
                    if handles.len() >= ENTRY_LIMIT { return Err("Too many plugin members to pin for move".into()); }
                    let handle = OpenOptions::new().read(true).access_mode(0x10000|0x80).share_mode(1|2|4).open(&path)
                        .map_err(|error| format!("Plugin member is in use or unavailable for uninstall: {error}"))?;
                    if !opened_file_within_workspace(&handle,root) { return Err("Plugin move handle escaped its selected package".into()); }
                    handles.push(handle);
                }
            }
            Ok(())
        }
        let mut handles = Vec::new();
        collect(root,root,&mut handles)?;
        Ok(handles)
    }
    #[cfg(not(windows))] { let _ = root; Ok(Vec::new()) }
}

impl PluginManager {
    pub(crate) fn initialize(&self,directory: &Path,app_data: &Path) -> Result<Arc<PluginSnapshot>,String> {
        let mut inner = self.inner.lock().map_err(|_| "Plugin manager lock is poisoned")?;
        if inner.setup.is_some() || inner.error.is_some() { return Err("Plugin manager was already initialized".into()); }
        let prepared = (|| -> Result<(Setup,Result<Arc<PluginSnapshot>,String>),String> {
            let directory = make_private_directory(directory)?;
            let app_data = resolved_directory(app_data)?;
            let private = make_private_directory(&app_data.join("plugin-manager"))?;
            let mut setup = Setup { directory,app_data,private,active:BTreeSet::new(),startup_plan:String::new(),warnings:vec![] };
            let _guard = StateLock::acquire(&setup)?;
            let (mut state,_) = load_state(&setup)?;
            let pending: Vec<String> = state.entries.iter().filter(|(_,policy)| policy.pending_uninstall).map(|(folder,_)| folder.clone()).collect();
            let mut changed = false;
            for folder in pending {
                let result = (|| -> Result<(),String> {
                    let source = target(&setup,&folder)?;
                    if safe_metadata(&source)?.is_none() { return Ok(()); }
                    let inspection = inspect(&setup,&folder,&state,"");
                    if !inspection.view.can_manage { return Err(inspection.view.error.unwrap_or_else(|| "Plugin folder cannot be moved safely".into())); }
                    if state.entries[&folder].uninstall_identity.as_deref() != Some(inspection.identity.as_str()) {
                        return Err("Pending plugin folder changed; it was not moved".into());
                    }
                    validate_tree(&source,0,&mut 0)?;
                    let move_handles = move_access(&source)?;
                    drop(move_handles);
                    move_to_backup(&setup,&source,"trash")?;
                    Ok(())
                })();
                match result {
                    Ok(()) => { state.entries.remove(&folder); changed = true; },
                    Err(error) => {
                        let message = bounded(&format!("Pending uninstall {folder}: {error}"),4093);
                        setup.warnings.push(message.clone());
                        let policy = state.entries.get_mut(&folder).unwrap();
                        if policy.error.as_deref() != Some(message.as_str()) { policy.error = Some(message); changed = true; }
                    },
                }
            }
            if changed { save_state(&setup,&mut state)?; }
            let (_,state_sha) = load_state(&setup)?;
            let entries = installed(&setup,&state,&state_sha)?;
            setup.startup_plan = plan(&entries);
            let exclusions = state.entries.iter().filter(|(_,policy)| !policy.enabled || policy.pending_uninstall).map(|(folder,_)| folder.clone()).collect();
            let captured = PluginSnapshot::capture_with_exclusions(&setup.directory,&setup.app_data,&exclusions).map(Arc::new);
            match &captured {
                Ok(snapshot) => setup.active = snapshot.captured_folders().clone(),
                Err(error) => setup.warnings.push(format!("Startup plugin snapshot failed: {error}; manage packages here and restart")),
            }
            Ok((setup,captured))
        })();
        match prepared {
            Ok((setup,captured)) => { inner.setup = Some(setup); captured },
            Err(error) => { let error = format!("Plugin manager {}: {error}",directory.display()); inner.error = Some(error.clone()); Err(error) },
        }
    }
    fn setup(inner: &Inner) -> Result<&Setup,String> { inner.setup.as_ref().ok_or_else(|| inner.error.clone().unwrap_or_else(|| "Plugin manager is not initialized".into())) }
    pub(crate) fn list(&self) -> Result<PluginView,String> {
        let inner = self.inner.lock().map_err(|_| "Plugin manager lock is poisoned")?;
        let setup = Self::setup(&inner)?;
        let _guard = StateLock::acquire(setup)?;
        let (state,state_sha) = load_state(setup)?;
        view(setup,&state,&state_sha)
    }
    fn change_validated(&self,folder: &str,revision: &str,enabled: Option<bool>,
        validator: impl FnOnce(&[PathBuf],&Path)->Result<(),String>) -> Result<PluginView,String> {
        let inner = self.inner.lock().map_err(|_| "Plugin manager lock is poisoned")?;
        let setup = Self::setup(&inner)?;
        let _guard = StateLock::acquire(setup)?;
        let (mut state,state_sha) = load_state(setup)?;
        let inspected = inspect(setup,folder,&state,&state_sha);
        if !inspected.view.can_manage { return Err(inspected.view.error.unwrap_or_else(|| "Plugin folder is not manageable".into())); }
        if inspected.view.revision != revision { return Err("Plugin view is stale; refresh before changing this folder".into()); }
        if inspected.view.pending_uninstall && enabled != Some(false) {
            return Err("Plugin uninstall is pending; use set_enabled(false) to cancel it while keeping this package disabled".into());
        }
        if enabled == Some(true) {
            let entries = installed(setup,&state,&state_sha)?;
            if !inspected.view.enabled && entries.iter().filter(|item| item.view.enabled && !item.view.pending_uninstall).count() >= 16 {
                return Err("At most 16 packages may be enabled for the next startup".into());
            }
            let roots: Vec<PathBuf> = entries.iter().filter(|item| !item.view.pending_uninstall
                && (item.view.enabled || item.view.folder == folder)).map(|item| target(setup,&item.view.folder)).collect::<Result<_,_>>()?;
            let operation = unique_directory(&setup.private.join("staging"),"enable-validation")?;
            let validation = validator(&roots,&operation);
            let cleanup = move_to_backup(setup,&operation,"cleanup");
            validation?;
            cleanup.map_err(|error| format!("Static validation passed but private cleanup is pending: {error}"))?;
        }
        if enabled.is_none() { validate_tree(&target(setup,folder)?,0,&mut 0)?; }
        state.entries.insert(folder.into(),Policy { enabled:enabled.unwrap_or(false),pending_uninstall:enabled.is_none(),
            uninstall_identity:if enabled.is_none() { Some(inspected.identity) } else { None },error:None });
        save_state(setup,&mut state)?;
        let (state,state_sha) = load_state(setup)?;
        view(setup,&state,&state_sha)
    }
    pub(crate) fn set_enabled(&self,folder: &str,revision: &str,enabled: bool) -> Result<PluginView,String> {
        self.change_validated(folder,revision,Some(enabled),validate_roots)
    }
    pub(crate) fn uninstall(&self,folder: &str,revision: &str) -> Result<PluginView,String> {
        self.change_validated(folder,revision,None,validate_roots)
    }

    pub(crate) fn install(&self,source_directory: &Path,trusted: bool) -> Result<PluginView,String> {
        self.install_validated(source_directory,trusted,validate_roots)
    }
    fn install_validated(&self,source_directory: &Path,trusted: bool,
        validator: impl FnOnce(&[PathBuf],&Path)->Result<(),String>) -> Result<PluginView,String> {
        if !trusted { return Err("Installing native code requires explicit trusted=true".into()); }
        let inner = self.inner.lock().map_err(|_| "Plugin manager lock is poisoned")?;
        let setup = Self::setup(&inner)?;
        let _guard = StateLock::acquire(setup)?;
        let (mut state,state_sha) = load_state(setup)?;
        let source = resolved_directory(source_directory)?;
        if !safe_metadata(&source)?.is_some_and(|meta| meta.is_dir()) { return Err("Select an existing complete plugin package folder".into()); }
        if source.starts_with(&setup.directory) || setup.directory.starts_with(&source)
            || source.starts_with(&setup.private) || setup.private.starts_with(&source) {
            return Err("Installation source must be outside the managed plugin and private operation directories".into());
        }
        let manifest_bytes = read_independent_file(&source.join("manifest.json"),&source,MANIFEST_LIMIT)?;
        let manifest = crate::graph_files::strict_json(&manifest_bytes).map_err(|_| "Source manifest is invalid JSON")?;
        let id = manifest.get("plugin_id").and_then(Value::as_str).filter(|id| id.len() <= 128 && folder_name(id)).ok_or("Source manifest has an unsafe plugin ID")?;
        let entry = manifest.get("entry").and_then(Value::as_str).filter(|entry| folder_name(entry) && entry.ends_with(".dll")).ok_or("Source manifest has an unsafe DLL entry filename")?;
        let hashes = package_members(&manifest,&source,id,entry)?;
        let entries = installed(setup,&state,&state_sha)?;
        if entries.iter().any(|item| item.view.plugin_id.as_deref() == Some(id)) || safe_metadata(&target(setup,id)?)?.is_some() {
            return Err("That plugin ID or destination already exists; it will not be overwritten".into());
        }
        if entries.iter().filter(|item| item.view.enabled && !item.view.pending_uninstall).count() >= 16 { return Err("At most 16 packages may be enabled for the next startup".into()); }
        let operation = unique_directory(&setup.private.join("staging"),"install")?;
        let result = (|| -> Result<(),String> {
            let packages = make_private_directory(&operation.join("packages"))?;
            let staged = make_private_directory(&packages.join("candidate"))?;
            copy_verified(&source.join("manifest.json"),&source,&staged.join("manifest.json"),MANIFEST_LIMIT as u64,&sha(&manifest_bytes))?;
            for (name,(hash,limit)) in &hashes {
                let destination = staged.join(name);
                let mut parent = staged.clone();
                for segment in name.split('/').take(name.split('/').count()-1) {
                    parent = make_private_directory(&parent.join(segment))?;
                }
                copy_verified(&source.join(name),&source,&destination,*limit,hash)?;
            }
            let mut roots: Vec<PathBuf> = entries.iter().filter(|item| item.view.enabled && !item.view.pending_uninstall)
                .map(|item| target(setup,&item.view.folder)).collect::<Result<_,_>>()?;
            roots.push(staged.clone());
            validator(&roots,&operation)?;
            // Reserve a disabled policy before publication. If the final state write
            // fails, an otherwise complete new package remains disabled next startup.
            state.entries.insert(id.into(),Policy { enabled:false,..Policy::default() });
            save_state(setup,&mut state)?;
            let destination = target(setup,id)?;
            move_new(&staged,&destination)?;
            state.entries.get_mut(id).unwrap().enabled = true;
            save_state(setup,&mut state)?;
            Ok(())
        })();
        let cleanup = move_to_backup(setup,&operation,"cleanup");
        if let Err(error) = result {
            return Err(match cleanup { Ok(_) => error,Err(cleanup) => format!("{error}; private stage retained for cleanup: {cleanup}") });
        }
        let (state,state_sha) = load_state(setup)?;
        let mut result = view(setup,&state,&state_sha)?;
        if let Err(error) = cleanup { result.warnings.push(format!("Installation completed; private stage cleanup is pending: {error}")); }
        Ok(result)
    }
}

fn resource_path(name: &str) -> bool {
    let parts: Vec<_> = name.split('/').collect();
    name.len() <= 256 && (2..=4).contains(&parts.len()) && parts[0] == "resources"
        && parts.iter().all(|part| part.len() <= 128 && folder_name(part)
            && part.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte)))
}
fn package_members(manifest: &Value,source: &Path,id: &str,entry: &str) -> Result<BTreeMap<String,(String,u64)>,String> {
    let version = manifest.get("schema_version").and_then(Value::as_u64).ok_or("Source manifest schema is missing")?;
    if version != 1 && version != 2 { return Err("Unsupported source manifest schema".into()); }
    if version == 2 && (entry.len() > 128 || !entry.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))) {
        return Err("Source v2 entry must be an ASCII safe segment".into());
    }
    if manifest.get("nodes").and_then(Value::as_str) != Some("nodes.json") { return Err("Source nodes must be nodes.json".into()); }
    let files = manifest.get("files").and_then(Value::as_array).ok_or("Source files are missing")?;
    if files.len() < 2 || files.len() > 64 || (version == 1 && files.len() != 2) { return Err("Source manifest has an invalid hashed file count".into()); }
    let mut hashes = BTreeMap::new();
    let mut names = BTreeSet::from(["manifest.json".to_owned()]);
    let mut directories = BTreeSet::new();
    let mut entries = 0;
    let mut nodes = 0;
    for item in files {
        let object = item.as_object().ok_or("Source file must be an object")?;
        if object.len() != if version == 1 { 2 } else { 3 }
            || object.keys().any(|key| key != "path" && key != "sha256" && !(version == 2 && key == "role")) {
            return Err("Source hashed file has unsupported fields".into());
        }
        let name = item.get("path").and_then(Value::as_str).ok_or("Source file path is missing")?;
        let hash = item.get("sha256").and_then(Value::as_str).filter(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())).ok_or("Source file SHA-256 is invalid")?;
        let role = if version == 1 { if name == entry { "entry" } else if name == "nodes.json" { "nodes" } else { "" } }
            else { item.get("role").and_then(Value::as_str).unwrap_or("") };
        let limit = match role {
            "entry" if name == entry => { entries += 1; DLL_LIMIT },
            "nodes" if name == "nodes.json" => { nodes += 1; NODES_LIMIT as u64 },
            "dependency" if version == 2 && name.len() <= 128 && folder_name(name)
                && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
                && name.starts_with(&format!("{id}.")) && name.to_ascii_lowercase().ends_with(".dll") => DLL_LIMIT,
            "resource" if version == 2 && resource_path(name) => DLL_LIMIT,
            _ => return Err("Source file has an invalid path or role".into()),
        };
        if !names.insert(name.to_ascii_lowercase()) { return Err("Source package contains case-insensitive path collisions".into()); }
        let mut components: Vec<_> = name.split('/').collect();
        components.pop();
        while !components.is_empty() { directories.insert(components.join("/")); components.pop(); }
        hashes.insert(name.to_owned(),(hash.to_owned(),limit));
    }
    if entries != 1 || nodes != 1 { return Err("Source package must contain exactly one entry and nodes file".into()); }
    let inventory = files_metadata(source)?;
    let mut total = 0u64;
    let mut seen = BTreeSet::new();
    for item in inventory {
        let name = item["name"].as_str().ok_or("Invalid source inventory name")?;
        if !seen.insert(name.to_ascii_lowercase()) { return Err("Source tree contains case-insensitive path collisions".into()); }
        if item["directory"] == true {
            if !directories.contains(name) { return Err("Source contains an undeclared directory".into()); }
        } else {
            let size = item["bytes"].as_u64().ok_or("Invalid source file size")?;
            let limit = if name == "manifest.json" { MANIFEST_LIMIT as u64 }
                else { hashes.get(name).ok_or("Source contains an undeclared file")?.1 };
            if size > limit { return Err("Source member exceeds its size limit".into()); }
            if name != "manifest.json" { total = total.checked_add(size).ok_or("Source size overflow")?; }
            if total > PACKAGE_LIMIT { return Err("Source package exceeds 1 GiB".into()); }
        }
    }
    if seen.len() != hashes.len()+directories.len()+1 { return Err("Source package is missing declared members".into()); }
    Ok(hashes)
}

fn copy_verified(source: &Path,source_root: &Path,destination: &Path,limit: u64,expected: &str) -> Result<(),String> {
    check_chain(source)?; check_chain(destination)?;
    let before = safe_metadata(source)?.ok_or("Source package member is missing")?;
    if !before.is_file() || before.len() > limit || is_hardlinked(source,&before) { return Err("Source package member is linked, unsupported or too large".into()); }
    let mut input = File::open(source).map_err(|error| error.to_string())?;
    if !opened_file_within_workspace(&input,source_root) { return Err("Source package member escaped its selected folder".into()); }
    let mut output = OpenOptions::new().write(true).create_new(true).open(destination).map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8;64*1024];
    loop {
        let count = input.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 { break; }
        total = total.checked_add(count as u64).ok_or("Package copy size overflow")?;
        if total > limit { return Err("Package member grew beyond its copy limit".into()); }
        output.write_all(&buffer[..count]).map_err(|error| error.to_string())?;
        digest.update(&buffer[..count]);
    }
    output.sync_all().map_err(|error| error.to_string())?;
    drop(output);
    let after = input.metadata().map_err(|error| error.to_string())?;
    check_chain(source)?;
    if total != before.len() || after.len() != before.len() || after.modified().ok() != before.modified().ok()
        || !opened_file_within_workspace(&input,source_root) || format!("{:x}",digest.finalize()) != expected {
        return Err("Package member changed or failed its manifest SHA-256 verification".into());
    }
    let mut staged = File::open(destination).map_err(|error| error.to_string())?;
    if !opened_file_within_workspace(&staged,destination.parent().ok_or("Stage has no parent")?) { return Err("Stage file escaped its private package folder".into()); }
    let mut digest = Sha256::new();
    let mut staged_bytes = 0u64;
    while let Ok(count) = staged.read(&mut buffer) {
        if count == 0 { return if format!("{:x}",digest.finalize()) == expected { Ok(()) } else { Err("Copied package member hash differs".into()) }; }
        staged_bytes = staged_bytes.checked_add(count as u64).ok_or("Stage copy size overflow")?;
        if staged_bytes > limit { return Err("Copied stage grew beyond its file limit".into()); }
        digest.update(&buffer[..count]);
    }
    Err("Cannot re-read copied package member".into())
}

fn validate_roots(roots: &[PathBuf],operation: &Path) -> Result<(),String> {
    validate_roots_with_executable(roots,operation,&fixed_sidecar()?)
}
fn validate_roots_with_executable(roots: &[PathBuf],operation: &Path,executable: &Path) -> Result<(),String> {
    let validation = make_private_directory(&operation.join("validation"))?;
    let workspace = make_private_directory(&operation.join("workspace"))?;
    let snapshot = Arc::new(PluginSnapshot::validation_roots(roots,&validation)?);
    let sidecar = Connection::spawn_with_plugins(executable,workspace,format!("plugin-install-{}",NEXT.fetch_add(1,Ordering::Relaxed)),
        false,false,Arc::new(|_| {}),snapshot)?;
    let result = sidecar.request(json!({"op":"capabilities"}));
    let stopped = sidecar.shutdown("Static plugin installation validation complete");
    stopped.map_err(|error| format!("Static validation sidecar cleanup failed: {error}"))?;
    let result = result?;
    let report = &result["data"]["plugins"];
    if result["success"] != true || !report["errors"].as_array().is_some_and(|errors| errors.is_empty())
        || !report["available"].as_array().is_some_and(|items| items.len() == roots.len()) {
        let detail = report["errors"].as_array().and_then(|errors| errors.first()).map(|error| format!(" [{}] {} (package {})",
            bounded(error["code"].as_str().unwrap_or("plugin_validation_failed"),128),
            bounded(error["message"].as_str().unwrap_or("Static package validation failed"),512),
            bounded(error["package"].as_str().unwrap_or("unknown"),256))).unwrap_or_default();
        return Err(format!("The fixed sidecar rejected the complete next-startup plugin set; no native DLL was executed.{detail}"));
    }
    Ok(())
}

#[tauri::command]
pub async fn plugin_manager_list(manager: tauri::State<'_,Arc<PluginManager>>) -> Result<PluginView,String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.list()).await.map_err(|_| "Plugin list task failed".to_owned())?
}
#[tauri::command]
pub async fn plugin_manager_install(manager: tauri::State<'_,Arc<PluginManager>>,source_directory: String,trusted: bool) -> Result<PluginView,String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.install(Path::new(&source_directory),trusted)).await.map_err(|_| "Plugin install task failed".to_owned())?
}
#[tauri::command]
pub async fn plugin_manager_set_enabled(manager: tauri::State<'_,Arc<PluginManager>>,folder: String,revision: String,enabled: bool) -> Result<PluginView,String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.set_enabled(&folder,&revision,enabled)).await.map_err(|_| "Plugin policy update task failed".to_owned())?
}
#[tauri::command]
pub async fn plugin_manager_uninstall(manager: tauri::State<'_,Arc<PluginManager>>,folder: String,revision: String) -> Result<PluginView,String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.uninstall(&folder,&revision)).await.map_err(|_| "Plugin uninstall task failed".to_owned())?
}

#[cfg(test)]
#[path="plugin_manager_tests.rs"]
mod tests;
