//! One app-startup selection of manually installed trusted native packages.
//! Rust hashes manifests only; DLL loading and package validation belong to the fixed sidecar.
use crate::run_records::{check_chain,ensure_dir,is_hardlinked,opened_file_within_workspace,safe_metadata};
use serde::Serialize;
use sha2::{Digest,Sha256};
use std::fs::{self,File,OpenOptions};
use std::io::{Read,Write};
use std::path::{Component,Path,PathBuf};
use std::sync::atomic::{AtomicU64,Ordering};
use std::time::{Duration,Instant};

const MANIFEST_LIMIT: usize = 64*1024;
const SNAPSHOT_LIMIT: usize = 64*1024;
const PACKAGE_LIMIT: usize = 16;
const DIRECTORY_LIMIT: usize = 128;
static NEXT_SNAPSHOT: AtomicU64 = AtomicU64::new(0);

#[derive(Debug,Serialize)]
struct PackageIdentity { root: String,manifest_sha256: String }
#[derive(Serialize)]
struct SnapshotDocument { schema_version: u32,packages: Vec<PackageIdentity> }

#[derive(Debug)]
struct StoredSnapshot { path: PathBuf,sha256: String,storage_root: PathBuf }

#[derive(Debug,Default)]
pub(crate) struct PluginSnapshot {
    plugin_directory: Option<PathBuf>,
    app_data: Option<PathBuf>,
    stored: Option<StoredSnapshot>,
}

#[derive(Debug)]
pub(crate) struct PluginLaunch {
    pub snapshot_path: PathBuf,
    pub snapshot_sha256: String,
    pub data_root: PathBuf,
}

fn digest(bytes: &[u8]) -> String { format!("{:x}",Sha256::digest(bytes)) }

// Resolve a not-yet-created app directory without creating it or following links.
fn resolved_directory(path: &Path) -> Result<PathBuf,String> {
    if !path.is_absolute() || path.file_name().is_none()
        || path.components().any(|part| matches!(part,Component::ParentDir | Component::CurDir)) {
        return Err("Plugin storage paths must be normalized absolute non-root directories".into());
    }
    check_chain(path)?;
    let mut existing = path;
    let mut missing = Vec::new();
    while safe_metadata(existing)?.is_none() {
        missing.push(existing.file_name().ok_or("Invalid plugin storage path")?.to_os_string());
        existing = existing.parent().ok_or("Invalid plugin storage path")?;
    }
    if !safe_metadata(existing)?.is_some_and(|meta| meta.is_dir()) {
        return Err("Plugin storage parent is not a normal directory".into());
    }
    let mut result = fs::canonicalize(existing).map_err(|error| format!("Cannot resolve plugin directory: {error}"))?;
    check_chain(&result)?;
    for part in missing.iter().rev() { result.push(part); }
    if result.to_str().is_none() { return Err("Plugin directory path must be valid Unicode".into()); }
    Ok(result)
}

fn make_private_directory(path: &Path) -> Result<PathBuf,String> {
    let resolved = resolved_directory(path)?;
    let mut existing = resolved.as_path();
    let mut missing = Vec::new();
    while safe_metadata(existing)?.is_none() {
        missing.push(existing.file_name().ok_or("Invalid plugin data directory")?.to_os_string());
        existing = existing.parent().ok_or("Invalid plugin data directory")?;
    }
    let mut current = existing.to_path_buf();
    for part in missing.iter().rev() { current.push(part); ensure_dir(&current)?; }
    check_chain(&current)?;
    fs::canonicalize(current).map_err(|error| format!("Cannot resolve private plugin storage: {error}"))
}

fn read_independent_file(path: &Path,root: &Path,limit: usize) -> Result<Vec<u8>,String> {
    check_chain(path)?;
    let before = safe_metadata(path)?.ok_or("Plugin manifest or snapshot file is missing")?;
    if !before.is_file() || is_hardlinked(path,&before) { return Err("Plugin files must be independent regular files".into()); }
    if before.len() > limit as u64 { return Err("Plugin manifest or snapshot exceeds 64 KiB".into()); }
    let mut file = File::open(path).map_err(|error| format!("Cannot open plugin metadata: {error}"))?;
    let opened = file.metadata().map_err(|error| format!("Cannot inspect opened plugin metadata: {error}"))?;
    if !opened_file_within_workspace(&file,root) || !opened.is_file()
        || opened.len() != before.len() || opened.modified().ok() != before.modified().ok() {
        return Err("Plugin metadata changed or escaped its selected package".into());
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file).take(limit as u64+1).read_to_end(&mut bytes)
        .map_err(|error| format!("Cannot read plugin metadata: {error}"))?;
    let after = file.metadata().map_err(|error| format!("Cannot inspect plugin metadata after read: {error}"))?;
    check_chain(path)?;
    if bytes.len() > limit || bytes.len() as u64 != before.len() || after.len() != before.len()
        || after.modified().ok() != before.modified().ok() || !opened_file_within_workspace(&file,root)
        || safe_metadata(path)?.is_none_or(|meta| !meta.is_file() || meta.len() != before.len()
            || meta.modified().ok() != before.modified().ok() || is_hardlinked(path,&meta)) {
        return Err("Plugin metadata changed while its identity was captured".into());
    }
    Ok(bytes)
}

impl PluginSnapshot {
    pub(crate) fn capture(plugin_directory: &Path,app_data: &Path) -> Result<Self,String> {
        let directory = resolved_directory(plugin_directory)?;
        let app_data = resolved_directory(app_data)?;
        // Prepare the displayed installation directory on first startup. Reuse
        // component-by-component checks; do not install packages or create a
        // runtime snapshot/data tree when the directory is empty.
        let directory = make_private_directory(&directory)?;
        let mut selected = Self { plugin_directory:Some(directory.clone()),app_data:Some(app_data.clone()),stored:None };
        let deadline = Instant::now()+Duration::from_secs(2);
        let mut roots = Vec::new();
        for (index,item) in fs::read_dir(&directory).map_err(|error| format!("Cannot scan installed plugins: {error}"))?.enumerate() {
            if index >= DIRECTORY_LIMIT || Instant::now() >= deadline { return Err("Installed plugin directory exceeds its bounded startup scan".into()); }
            let item = item.map_err(|error| format!("Cannot inspect installed plugin entry: {error}"))?;
            let path = item.path();
            let meta = safe_metadata(&path)?.ok_or("Installed plugin entry disappeared")?;
            if !meta.is_dir() { continue; }
            if roots.len() == PACKAGE_LIMIT { return Err("At most 16 directly installed plugin packages are supported".into()); }
            let root = resolved_directory(&path)?;
            if root.parent() != Some(directory.as_path()) { return Err("Plugin package must be a direct child of the fixed plugin directory".into()); }
            roots.push(root);
        }
        roots.sort();
        let mut packages = Vec::new();
        for root in roots {
            if Instant::now() >= deadline { return Err("Installed plugin snapshot exceeded its startup time budget".into()); }
            let bytes = read_independent_file(&root.join("manifest.json"),&root,MANIFEST_LIMIT)
                .map_err(|error| format!("Plugin package {}: {error}",root.display()))?;
            packages.push(PackageIdentity { root:root.to_str().ok_or("Plugin root is not Unicode")?.into(),manifest_sha256:digest(&bytes) });
        }
        check_chain(&directory)?;
        if packages.is_empty() { return Ok(selected); }
        let bytes = serde_json::to_vec(&SnapshotDocument { schema_version:1,packages })
            .map_err(|error| format!("Cannot encode plugin snapshot: {error}"))?;
        if bytes.len() > SNAPSHOT_LIMIT { return Err("Plugin snapshot exceeds 64 KiB".into()); }
        let sha256 = digest(&bytes);
        let storage_root = make_private_directory(&app_data.join("plugin-snapshots"))?;
        for _ in 0..32 {
            let serial = NEXT_SNAPSHOT.fetch_add(1,Ordering::Relaxed);
            let path = storage_root.join(format!("{sha256}-{}-{serial}.json",std::process::id()));
            check_chain(&path)?;
            let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("Cannot create private plugin snapshot: {error}")),
            };
            file.write_all(&bytes).and_then(|_| file.sync_all()).map_err(|error| format!("Cannot persist plugin snapshot: {error}"))?;
            drop(file);
            if read_independent_file(&path,&storage_root,SNAPSHOT_LIMIT)? != bytes {
                return Err("Private plugin snapshot did not retain its captured bytes".into());
            }
            selected.stored = Some(StoredSnapshot { path,sha256,storage_root });
            return Ok(selected);
        }
        Err("Cannot reserve a unique private plugin snapshot".into())
    }

    pub(crate) fn plugin_directory(&self) -> Option<&Path> { self.plugin_directory.as_deref() }

    pub(crate) fn launch_for(&self,workspace: &Path) -> Result<Option<PluginLaunch>,String> {
        let Some(stored) = &self.stored else { return Ok(None); };
        let bytes = read_independent_file(&stored.path,&stored.storage_root,SNAPSHOT_LIMIT)?;
        if digest(&bytes) != stored.sha256 { return Err("Private plugin snapshot identity changed; restart the application".into()); }
        let workspace = fs::canonicalize(workspace).map_err(|error| format!("Cannot identify plugin workspace: {error}"))?;
        if !workspace.is_dir() { return Err("Plugin workspace is not a directory".into()); }
        let identity = if cfg!(windows) { workspace.to_string_lossy().to_lowercase() }
            else { workspace.to_string_lossy().into_owned() };
        let app_data = self.app_data.as_deref().ok_or("Plugin app data root is unavailable")?;
        let data_root = make_private_directory(&app_data.join("plugin-data").join(digest(identity.as_bytes())))?;
        Ok(Some(PluginLaunch { snapshot_path:stored.path.clone(),snapshot_sha256:stored.sha256.clone(),data_root }))
    }
}

#[cfg(test)]
#[path="plugin_snapshot_tests.rs"]
mod tests;
