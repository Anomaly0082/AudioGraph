use crate::run_records::{RunDraft, RunFileDraft, RunStore};
use crate::tool_workspaces::ToolWorkspaces;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture {
    root: PathBuf,
    user: PathBuf,
    app: PathBuf,
    spaces: ToolWorkspaces,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!(
            "audioprocess-run-review-{}-{nanos}-{}",
            std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let user = root.join("user");
        let app = root.join("app");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&user).unwrap();
        fs::create_dir(&app).unwrap();
        let spaces = ToolWorkspaces::new(&user, &app).unwrap();
        Self { root, user, app, spaces }
    }

    fn draft(&self, files: Vec<RunFileDraft>) -> RunDraft {
        RunDraft {
            kind: "graph".into(), origin: "manual".into(), parent_id: None,
            name: "Snapshot review".into(),
            configuration: json!({"graph":{"schema_version":1,"nodes":[]},"options":{"probe":true}}),
            files,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { fs::remove_dir_all(&self.root).unwrap(); }
}

fn input_file() -> RunFileDraft {
    RunFileDraft { space: "user".into(), path: "input.wav".into(), role: "input".into() }
}

#[test]
fn completed_record_and_configuration_survive_store_restart() {
    let fixture = Fixture::new();
    fs::write(fixture.user.join("input.wav"), b"RIFF-A").unwrap();
    let store = RunStore::default();
    let draft = fixture.draft(vec![input_file()]);
    let configuration = draft.configuration.clone();
    let started = store.begin(&fixture.spaces, &fixture.app, draft).unwrap();
    let finished = store.finish(&fixture.spaces, &fixture.app, &started.id,
        "succeeded", Some(json!({"ok":true})), None, vec![]).unwrap();
    assert_eq!(finished.configuration, configuration);

    let reopened = RunStore::default();
    let loaded = reopened.load(&fixture.spaces, &fixture.app, &started.id).unwrap();
    assert_eq!(loaded.state, "succeeded");
    assert_eq!(loaded.configuration, configuration);
    assert_eq!(loaded.result, Some(json!({"ok":true})));
    assert_eq!(reopened.list(&fixture.spaces, &fixture.app).unwrap().records.len(), 1);
    assert!(reopened.finish(&fixture.spaces, &fixture.app, &started.id,
        "failed", None, Some("late failure".into()), vec![]).is_err());
    assert_eq!(reopened.load(&fixture.spaces, &fixture.app, &started.id).unwrap().state, "succeeded");
}

#[test]
fn file_checks_distinguish_available_changed_and_missing() {
    let fixture = Fixture::new();
    let input = fixture.user.join("input.wav");
    fs::write(&input, b"RIFF-A").unwrap();
    let store = RunStore::default();
    let started = store.begin(&fixture.spaces, &fixture.app,
        fixture.draft(vec![input_file()])).unwrap();
    store.finish(&fixture.spaces, &fixture.app, &started.id,
        "succeeded", None, None, vec![]).unwrap();
    let reopened = RunStore::default();
    assert_eq!(reopened.check_files(&fixture.spaces, &fixture.app, &started.id).unwrap()[0].status, "available");
    fs::write(&input, b"RIFF-B").unwrap();
    assert_eq!(reopened.check_files(&fixture.spaces, &fixture.app, &started.id).unwrap()[0].status, "changed");
    fs::remove_file(&input).unwrap();
    assert_eq!(reopened.check_files(&fixture.spaces, &fixture.app, &started.id).unwrap()[0].status, "missing");
}

#[test]
fn file_capture_rejects_paths_outside_workspace() {
    let fixture = Fixture::new();
    fs::write(fixture.root.join("outside.wav"), b"secret").unwrap();
    let store = RunStore::default();
    for path in ["../outside.wav", "sub/../../outside.wav"] {
        let draft = fixture.draft(vec![RunFileDraft {
            space: "user".into(), path: path.into(), role: "input".into(),
        }]);
        assert!(store.begin(&fixture.spaces, &fixture.app, draft).is_err());
    }
    assert!(store.list(&fixture.spaces, &fixture.app).unwrap().records.is_empty());
}
