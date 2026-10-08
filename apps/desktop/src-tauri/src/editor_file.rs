//! Manual editor saves only. The revision check is optimistic, not cross-process CAS.
use crate::tool_workspaces::{check_existing_chain, is_hardlinked, normal_metadata, replace_file, temp_file};
use sha2::{Digest, Sha256};
use std::{fs, io::{Read, Write}, path::{Path, PathBuf}, sync::Mutex};

pub(crate) const CONFLICT: &str = "文件已被外部修改或删除，请重新打开或另存为";
static SAVES: Mutex<()> = Mutex::new(());

pub(crate) fn revision(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }

pub(crate) fn safe_file(path: &Path) -> Result<(), String> {
    check_existing_chain(path)?;
    if let Some(meta) = normal_metadata(path)? {
        if !meta.is_file() || is_hardlinked(path, &meta) { return Err("Expected an independent regular file".into()); }
    }
    Ok(())
}

fn check_revision(path: &Path, expected: Option<&str>, limit: usize) -> Result<(), String> {
    safe_file(path)?;
    match expected {
        None => if normal_metadata(path)?.is_some() { return Err("Save as must create a new file; target already exists".into()); },
        Some(expected) => {
            let mut file = fs::File::open(path).map_err(|_| CONFLICT.to_owned())?;
            let parent = path.parent().ok_or(CONFLICT)?;
            if !crate::run_records::opened_file_within_workspace(&file,parent) { return Err(CONFLICT.into()); }
            let before = file.metadata().map_err(|_| CONFLICT.to_owned())?;
            if !before.is_file() || before.len() > limit as u64 { return Err(CONFLICT.into()); }
            let mut bytes = Vec::new();
            (&mut file).take((limit + 1) as u64).read_to_end(&mut bytes).map_err(|_| CONFLICT.to_owned())?;
            let after = file.metadata().map_err(|_| CONFLICT.to_owned())?;
            if bytes.len() > limit || bytes.len() as u64 != before.len() || after.len() != before.len() || after.modified().ok() != before.modified().ok()
                || !crate::run_records::opened_file_within_workspace(&file,parent) || revision(&bytes) != expected { return Err(CONFLICT.into()); }
        }
    }
    Ok(())
}

pub(crate) fn save<F>(bytes: &[u8], expected: Option<&str>, limit: usize, mut resolve: F) -> Result<(PathBuf,String), String>
where F: FnMut(bool) -> Result<PathBuf,String> {
    if bytes.len() > limit { return Err("Editor snapshot exceeds file size limit".into()); }
    if expected.is_some_and(|value| value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))) {
        return Err("Invalid editor revision".into());
    }
    let _guard = SAVES.lock().map_err(|_| "Editor save lock unavailable")?;
    let target = resolve(true)?;
    check_revision(&target, expected, limit)?;
    let (temp,mut file) = temp_file(target.parent().ok_or("File has no parent")?)?;
    let result = (|| {
        file.write_all(bytes).and_then(|_| file.sync_all()).map_err(|e| format!("Cannot write editor file: {e}"))?;
        drop(file);
        // Resolve and hash again immediately before publishing. Other processes can
        // still race this check; manual saves in this process share the mutex.
        let current = resolve(false)?;
        if current != target { return Err(CONFLICT.into()); }
        check_revision(&current, expected, limit)?;
        if expected.is_some() { replace_file(&temp,&target)?; }
        else { fs::hard_link(&temp,&target).map_err(|e| format!("Save as must create a new file: {e}"))?; }
        Ok((target,revision(bytes)))
    })();
    let _ = fs::remove_file(temp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64,Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn root() -> PathBuf {
        let path = std::env::temp_dir().join(format!("manual-editor-save-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        path
    }
    #[test]
    fn revisions_repeated_save_stale_create_and_deleted() {
        let root = root(); let path = root.join("snapshot.json");
        let (_,first) = save(b"first",None,100, |_| Ok(path.clone())).unwrap();
        assert_eq!(first,revision(b"first"));
        assert!(save(b"other",None,100, |_| Ok(path.clone())).is_err());
        let (_,second) = save(b"second",Some(&first),100, |_| Ok(path.clone())).unwrap();
        assert_ne!(first,second);
        assert_eq!(save(b"stale",Some(&first),100, |_| Ok(path.clone())).unwrap_err(),CONFLICT);
        assert_eq!(fs::read(&path).unwrap(),b"second");
        save(b"third",Some(&second),100, |_| Ok(path.clone())).unwrap();
        fs::remove_file(&path).unwrap();
        assert_eq!(save(b"deleted",Some(&second),100, |_| Ok(path.clone())).unwrap_err(),CONFLICT);
        assert_eq!(fs::read_dir(&root).unwrap().count(),0);
        fs::remove_dir(root).unwrap();
    }
    #[test]
    fn hardlink_and_failed_late_path_check_preserve_original() {
        let root = root(); let path = root.join("snapshot.json");
        fs::write(&path,b"original").unwrap();
        let linked = root.join("linked.json"); fs::hard_link(&path,&linked).unwrap();
        assert!(save(b"changed",Some(&revision(b"original")),100, |_| Ok(path.clone())).is_err());
        fs::remove_file(linked).unwrap();
        assert!(save(b"changed",Some(&revision(b"original")),100, |prepare| {
            if prepare { Ok(path.clone()) } else { Err("late path failure".into()) }
        }).is_err());
        assert_eq!(fs::read(&path).unwrap(),b"original");
        assert_eq!(fs::read_dir(&root).unwrap().count(),1);
        fs::remove_file(path).unwrap(); fs::remove_dir(root).unwrap();
    }
    #[cfg(windows)]
    #[test]
    fn locked_destination_preserves_original_and_cleans_temp() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = root(); let path = root.join("snapshot.json");
        fs::write(&path,b"original").unwrap();
        let locked = fs::OpenOptions::new().read(true).share_mode(1).open(&path).unwrap();
        assert!(save(b"changed",Some(&revision(b"original")),100, |_| Ok(path.clone())).is_err());
        assert_eq!(fs::read(&path).unwrap(),b"original");
        assert_eq!(fs::read_dir(&root).unwrap().count(),1);
        drop(locked); fs::remove_file(path).unwrap(); fs::remove_dir(root).unwrap();
    }
}
