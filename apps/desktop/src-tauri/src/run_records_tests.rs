use super::*;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_SERIAL: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    app_data: PathBuf,
    spaces: ToolWorkspaces,
}

impl Fixture {
    fn new() -> Self {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let serial = TEST_SERIAL.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "audioprocess_run_records_{}_{}_{}", std::process::id(), nonce, serial));
        let user = root.join("user");
        let app_data = root.join("app-data");
        fs::create_dir_all(&user).unwrap();
        fs::create_dir(&app_data).unwrap();
        let spaces = ToolWorkspaces::new(&user, &app_data).unwrap();
        Self { root, app_data, spaces }
    }

    fn draft(&self) -> RunDraft {
        RunDraft { kind: "graph".into(), origin: "manual".into(), parent_id: None,
            name: "Example graph".into(), configuration: json!({"graph":{"nodes":[]}}),
            files: vec![RunFileDraft { space: "user".into(), path: "input.wav".into(), role: "input".into() }] }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Fixture created this unique directory under the OS temporary directory.
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn records_are_outside_workspaces_and_terminal_state_is_final() {
    let fixture = Fixture::new();
    fs::write(fixture.spaces.user_root.join("input.wav"), b"RIFF sample").unwrap();
    let store = RunStore::default();
    let begun = store.begin(&fixture.spaces, &fixture.app_data, fixture.draft()).unwrap();
    let dir = storage_dir(&fixture.spaces, &fixture.app_data).unwrap();
    assert!(dir.starts_with(fs::canonicalize(&fixture.app_data).unwrap()));
    assert!(!dir.starts_with(&fixture.spaces.user_root));
    assert!(!dir.starts_with(&fixture.spaces.ai_root));
    assert!(record_path(&dir, &begun.id).unwrap().is_file());
    assert_eq!(begun.files[0].capture_status, "captured");
    assert_eq!(begun.files[0].size_bytes, Some(11));
    assert_eq!(store.load(&fixture.spaces, &fixture.app_data, &begun.id).unwrap().state, "running");

    let ended = store.finish(&fixture.spaces, &fixture.app_data, &begun.id, "succeeded",
        Some(json!({"ok":true})), None,
        vec![RunFileDraft { space: "user".into(), path: "output.wav".into(), role: "output".into() }]).unwrap();
    assert_eq!(ended.state, "succeeded");
    assert_eq!(ended.files[1].capture_status, "missing");
    assert!(ended.finished_at_ms.is_some());
    assert!(ended.duration_ms.is_some());
    assert!(store.finish(&fixture.spaces, &fixture.app_data, &begun.id, "failed", None,
        Some("late".into()), vec![]).is_err());
    assert_eq!(store.load(&fixture.spaces, &fixture.app_data, &begun.id).unwrap().state, "succeeded");
}

#[test]
fn prior_process_running_record_is_displayed_as_interrupted() {
    let fixture = Fixture::new();
    let first = RunStore::default();
    let begun = first.begin(&fixture.spaces, &fixture.app_data, fixture.draft()).unwrap();
    assert_eq!(first.load(&fixture.spaces, &fixture.app_data, &begun.id).unwrap().state, "running");
    let restarted = RunStore::default();
    assert_eq!(restarted.load(&fixture.spaces, &fixture.app_data, &begun.id).unwrap().state, "interrupted");
    assert_eq!(read_record(&storage_dir(&fixture.spaces, &fixture.app_data).unwrap(), &begun.id).unwrap().state, "running");
    assert!(restarted.finish(&fixture.spaces, &fixture.app_data, &begun.id, "succeeded", None, None, vec![]).is_err());
    assert_eq!(first.load(&fixture.spaces, &fixture.app_data, &begun.id).unwrap().state, "running");
}

#[test]
fn file_check_distinguishes_available_changed_missing_and_unverified() {
    let fixture = Fixture::new();
    let file = fixture.spaces.user_root.join("input.wav");
    fs::write(&file, b"audio A").unwrap();
    let store = RunStore::default();
    let begun = store.begin(&fixture.spaces, &fixture.app_data, fixture.draft()).unwrap();
    assert_eq!(store.check_files(&fixture.spaces, &fixture.app_data, &begun.id).unwrap()[0].status, "available");
    fs::write(&file, b"audio B").unwrap();
    assert_eq!(store.check_files(&fixture.spaces, &fixture.app_data, &begun.id).unwrap()[0].status, "changed");
    fs::remove_file(&file).unwrap();
    assert_eq!(store.check_files(&fixture.spaces, &fixture.app_data, &begun.id).unwrap()[0].status, "missing");
    let missing = store.begin(&fixture.spaces, &fixture.app_data, fixture.draft()).unwrap();
    fs::write(&file, b"later").unwrap();
    assert_eq!(store.check_files(&fixture.spaces, &fixture.app_data, &missing.id).unwrap()[0].status, "unverified");
}

#[test]
fn bad_record_is_warned_about_without_hiding_valid_history() {
    let fixture = Fixture::new();
    let store = RunStore::default();
    let good = store.begin(&fixture.spaces, &fixture.app_data, fixture.draft()).unwrap();
    let dir = storage_dir(&fixture.spaces, &fixture.app_data).unwrap();
    let bad_id = "a".repeat(64);
    fs::write(record_path(&dir, &bad_id).unwrap(), b"not json").unwrap();
    let list = store.list(&fixture.spaces, &fixture.app_data).unwrap();
    assert_eq!(list.records.len(), 1);
    assert_eq!(list.records[0].id, good.id);
    assert_eq!(list.records[0].state, "running");
    assert_eq!(list.warnings.len(), 1);
    assert!(list.warnings[0].contains(&bad_id));
}

#[test]
fn unsafe_references_are_rejected_before_any_record_is_created() {
    let fixture = Fixture::new();
    let store = RunStore::default();
    for path in ["../outside.wav", "/outside.wav", "C:/outside.wav", "a\\b.wav", ""] {
        let mut draft = fixture.draft();
        draft.files[0].path = path.into();
        assert!(store.begin(&fixture.spaces, &fixture.app_data, draft).is_err(), "accepted {path:?}");
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc/passwd", fixture.spaces.user_root.join("link.wav")).unwrap();
        let mut draft = fixture.draft();
        draft.files[0].path = "link.wav".into();
        assert_eq!(store.begin(&fixture.spaces, &fixture.app_data, draft).unwrap().files[0].capture_status, "unverified");
    }
    #[cfg(windows)]
    {
        let outside = fixture.root.join("outside.wav");
        fs::write(&outside, b"outside").unwrap();
        let link = fixture.spaces.user_root.join("link.wav");
        // Symlink creation can require developer mode or a specific Windows privilege.
        if std::os::windows::fs::symlink_file(&outside, &link).is_ok() {
            let mut draft = fixture.draft();
            draft.files[0].path = "link.wav".into();
            assert_eq!(store.begin(&fixture.spaces, &fixture.app_data, draft).unwrap().files[0].capture_status, "unverified");
        }
    }
    let original = fixture.spaces.user_root.join("original.wav");
    fs::write(&original, b"audio").unwrap();
    fs::hard_link(&original, fixture.spaces.user_root.join("hardlink.wav")).unwrap();
    let mut draft = fixture.draft();
    draft.files[0].path = "hardlink.wav".into();
    assert_eq!(store.begin(&fixture.spaces, &fixture.app_data, draft).unwrap().files[0].capture_status, "unverified");
}

#[test]
fn large_result_does_not_leave_run_active() {
    let fixture = Fixture::new();
    let store = RunStore::default();
    let begun = store.begin(&fixture.spaces, &fixture.app_data, fixture.draft()).unwrap();
    let oversized = json!({"samples":"x".repeat(600 * 1024)});
    let ended = store.finish(&fixture.spaces, &fixture.app_data, &begun.id, "succeeded",
        Some(oversized), None, vec![]).unwrap();
    assert_eq!(ended.state, "succeeded");
    assert!(ended.result.is_none());
    assert!(ended.error.is_none());
    assert!(ended.recording_warning.as_deref().unwrap().contains("omitted"));
    assert_eq!(store.load(&fixture.spaces, &fixture.app_data, &begun.id).unwrap().state, "succeeded");
}
