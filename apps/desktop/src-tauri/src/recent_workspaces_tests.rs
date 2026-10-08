use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Temporary(PathBuf);
impl Temporary {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("audio-recent-test-{}-{}", std::process::id(), SERIAL.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&p).unwrap(); Self(p)
    }
    fn storage(&self) -> PathBuf { self.0.join("config").join("recent-workspaces.json") }
}
impl Drop for Temporary { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

#[test]
fn reload_mru_cap_and_metadata_only_remove() {
    let temp = Temporary::new(); let path = temp.storage();
    let store = RecentWorkspaceStore::default();
    assert!(store.list(&path).unwrap().paths.is_empty());
    assert!(!path.parent().unwrap().exists());
    for i in 0..12 { store.remember(&path, &temp.0.join(format!("missing-{i}"))).unwrap(); }
    let reopened = RecentWorkspaceStore::default();
    let paths = reopened.list(&path).unwrap().paths;
    assert_eq!(paths.len(), 10);
    assert_eq!(paths[0], temp.0.join("missing-11").to_str().unwrap());
    reopened.remember(&path, &temp.0.join("missing-5")).unwrap();
    assert_eq!(reopened.list(&path).unwrap().paths[0], temp.0.join("missing-5").to_str().unwrap());
    assert!(!temp.0.join("missing-5").exists());
    let target = temp.0.join("deleted"); fs::create_dir(&target).unwrap();
    reopened.remember(&path, &target).unwrap(); fs::remove_dir(&target).unwrap();
    assert!(reopened.list(&path).unwrap().paths.contains(&target.to_str().unwrap().to_owned()));
    reopened.remove(&path, target.to_str().unwrap()).unwrap();
    assert!(!target.exists());
    assert!(!reopened.list(&path).unwrap().paths.contains(&target.to_str().unwrap().to_owned()));
    reopened.clear(&path).unwrap();
    assert!(store.list(&path).unwrap().paths.is_empty());
}

#[test]
fn corruption_preserved_until_explicit_clear() {
    let temp = Temporary::new(); let path = temp.storage();
    fs::create_dir(path.parent().unwrap()).unwrap(); fs::write(&path, b"broken").unwrap();
    let store = RecentWorkspaceStore::default();
    assert!(store.list(&path).is_err());
    assert!(store.remember(&path, &temp.0.join("uncreated")).is_err());
    assert!(store.remove(&path, temp.0.join("uncreated").to_str().unwrap()).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"broken");
    assert!(!temp.0.join("uncreated").exists());
    store.clear(&path).unwrap(); assert!(store.list(&path).unwrap().paths.is_empty());
}

#[test]
fn unsafe_metadata_target_is_not_reset() {
    let temp = Temporary::new(); let path = temp.storage();
    fs::create_dir_all(&path).unwrap();
    let store = RecentWorkspaceStore::default();
    assert!(store.clear(&path).is_err()); assert!(path.is_dir());
}

#[cfg(windows)]
#[test]
fn windows_case_dedup_preserves_latest_spelling() {
    let temp = Temporary::new(); let path = temp.storage(); let store = RecentWorkspaceStore::default();
    store.remember(&path, Path::new(r"C:\Example\Workspace")).unwrap();
    let result = store.remember(&path, Path::new(r"c:\example\WORKSPACE")).unwrap();
    assert_eq!(result.paths, vec![r"c:\example\WORKSPACE"]);
}
