use crate::tool_workspaces::ToolWorkspaces;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static SERIAL: AtomicU64 = AtomicU64::new(0);

struct Fixture { root: PathBuf, workspaces: ToolWorkspaces }

impl Fixture {
    fn new() -> Self {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("audioprocess_tool_workspaces_{}_{}_{}", std::process::id(), nonce, serial));
        fs::create_dir(&root).unwrap();
        let user = root.join("user");
        let app = root.join("app");
        fs::create_dir(&user).unwrap();
        fs::create_dir(&app).unwrap();
        let workspaces = ToolWorkspaces::new(&user, &app).unwrap();
        Self { root, workspaces }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // This path is a unique directory made by Fixture::new inside the OS temp directory.
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn roundtrip_copy_edit_read_list_and_export() {
    let fixture = Fixture::new();
    let ws = &fixture.workspaces;
    fs::write(ws.user_root.join("input.txt"), "source").unwrap();
    let same = ToolWorkspaces::new(&ws.user_root, &fixture.root.join("app")).unwrap();
    assert_eq!(same.ai_root, ws.ai_root);

    let copied = ws.dispatch("file_copy_to_ai", &json!({"source_space":"user","source_path":"input.txt","path":"nested/copy.txt"})).unwrap();
    assert_eq!(copied["space"], "ai");
    assert_eq!(fs::read(ws.ai_root.join("nested/copy.txt")).unwrap(), b"source");
    fs::write(ws.user_root.join("input.txt"), "changed").unwrap();
    assert_eq!(fs::read(ws.ai_root.join("nested/copy.txt")).unwrap(), b"source");

    ws.dispatch("file_write_text", &json!({"path":"nested/copy.txt","content":"replacement"})).unwrap();
    let read = ws.dispatch("file_read_text", &json!({"space":"ai","path":"nested/copy.txt"})).unwrap();
    assert_eq!(read["content"], "replacement");
    let listing = ws.dispatch("workspace_list", &json!({"space":"ai","path":"nested"})).unwrap();
    assert_eq!(listing["entries"][0]["name"], "copy.txt");

    let exported = ws.dispatch("file_export", &json!({"path":"nested/copy.txt","user_path":"result.txt"})).unwrap();
    assert_eq!(exported["space"], "user");
    assert_eq!(fs::read(ws.user_root.join("result.txt")).unwrap(), b"replacement");
    ws.dispatch("file_delete", &json!({"path":"nested/copy.txt"})).unwrap();
    ws.dispatch("file_delete", &json!({"path":"nested"})).unwrap();
    assert!(!ws.ai_root.join("nested").exists());
    ws.check_quota().unwrap();
}

#[test]
fn user_files_cannot_be_changed_or_overwritten() {
    let fixture = Fixture::new();
    let ws = &fixture.workspaces;
    fs::write(ws.user_root.join("kept.txt"), b"original").unwrap();
    ws.dispatch("file_write_text", &json!({"path":"sample.txt","content":"AI"})).unwrap();
    for call in [
        ("file_write_text", json!({"space":"user","path":"kept.txt","content":"changed"})),
        ("file_delete", json!({"space":"user","path":"kept.txt"})),
        ("file_copy_to_ai", json!({"source_space":"ai","source_path":"sample.txt","path":"sample.txt"})),
        ("file_export", json!({"path":"sample.txt","user_path":"kept.txt"})),
    ] {
        assert!(ws.dispatch(call.0, &call.1).is_err(), "{} was accepted", call.0);
    }
    assert_eq!(fs::read(ws.user_root.join("kept.txt")).unwrap(), b"original");
    assert_eq!(fs::read(ws.ai_root.join("sample.txt")).unwrap(), b"AI");
    assert!(ws.dispatch("file_export", &json!({"path":"sample.txt","user_path":"missing/result.txt"})).is_err());
    assert!(!ws.user_root.join("missing").exists());
}

#[test]
fn invalid_paths_and_arguments_are_rejected() {
    let fixture = Fixture::new();
    let ws = &fixture.workspaces;
    for path in ["", ".", "..", "a/../b", "a//b", "a/./b", "/absolute", "C:/drive", "C:relative", "//server/share", "\\\\?\\C:\\device", "a\\b", "con.txt", "LPT9", "a.", "a ", "a:b", "a\0b", "a?b"] {
        assert!(ws.dispatch("file_write_text", &json!({"path":path,"content":"x"})).is_err(), "accepted {path:?}");
    }
    assert!(ws.dispatch("workspace_list", &json!({"space":"user","path":""})).is_ok());
    assert!(ws.dispatch("file_delete", &json!({"path":""})).is_err());
    assert!(ws.dispatch("file_write_text", &json!({"path":"a","content":"b","extra":0})).is_err());
    assert!(ws.dispatch("file_write_text", &json!({"path":"a","content":7})).is_err());
    assert!(ws.dispatch("workspace_list", &json!({"space":"private"})).is_err());
    assert!(ws.checked_path("user", "../app", true).is_err());
}

#[test]
fn root_delete_nonempty_directory_and_limits_are_rejected() {
    let fixture = Fixture::new();
    let ws = &fixture.workspaces;
    ws.dispatch("file_write_text", &json!({"path":"folder/file.txt","content":"x"})).unwrap();
    assert!(ws.dispatch("file_delete", &json!({"path":"folder"})).is_err());
    assert!(ws.dispatch("file_delete", &json!({"path":""})).is_err());
    assert!(ws.ai_root.is_dir());
    assert!(ws.dispatch("file_write_text", &json!({"path":"large.txt","content":"x".repeat(64 * 1024 + 1)})).is_err());
    fs::write(ws.user_root.join("large.txt"), vec![b'a'; 64 * 1024 + 1]).unwrap();
    assert!(ws.dispatch("file_read_text", &json!({"space":"user","path":"large.txt"})).is_err());
    fs::write(ws.user_root.join("binary.txt"), [0xff, 0xfe]).unwrap();
    assert!(ws.dispatch("file_read_text", &json!({"space":"user","path":"binary.txt"})).is_err());
}

#[test]
fn atomic_replace_and_hardlinks_do_not_mutate_aliases() {
    let fixture = Fixture::new();
    let ws = &fixture.workspaces;
    ws.dispatch("file_write_text", &json!({"path":"one.txt","content":"first"})).unwrap();
    ws.dispatch("file_write_text", &json!({"path":"one.txt","content":"second"})).unwrap();
    assert_eq!(fs::read(ws.ai_root.join("one.txt")).unwrap(), b"second");
    let alias = fixture.root.join("outside-alias.txt");
    fs::hard_link(ws.ai_root.join("one.txt"), &alias).unwrap();
    assert!(ws.checked_path("ai", "one.txt", false).is_err());
    assert!(ws.dispatch("file_write_text", &json!({"path":"one.txt","content":"third"})).is_err());
    assert_eq!(fs::read(&alias).unwrap(), b"second");
    assert!(ws.check_quota().is_err());
}

#[test]
fn workspace_roots_must_not_overlap_application_data() {
    let fixture = Fixture::new();
    let app = fixture.root.join("app");
    assert!(ToolWorkspaces::new(&fixture.root, &app).is_err());
    assert!(ToolWorkspaces::new(&app, &app).is_err());
    assert!(ToolWorkspaces::new(&fixture.workspaces.ai_root, &app).is_err());
    let within_user = fixture.workspaces.user_root.join("private/new/app-data");
    assert!(ToolWorkspaces::new(&fixture.workspaces.user_root, &within_user).is_err());
    assert!(!within_user.exists());
}

#[test]
fn a_missing_application_data_directory_is_created_safely() {
    let fixture = Fixture::new();
    let new_data = fixture.root.join("fresh/deep/app-data");
    let ws = ToolWorkspaces::new(&fixture.workspaces.user_root, &new_data).unwrap();
    assert!(ws.ai_root.is_dir());
    assert!(ws.ai_root.starts_with(fs::canonicalize(new_data).unwrap()));
}

#[test]
fn original_and_canonical_user_paths_share_one_ai_workspace() {
    let fixture = Fixture::new();
    let original_user = fixture.root.join("user");
    let canonical_user = fs::canonicalize(&original_user).unwrap();
    let app_data = fixture.root.join("app");
    let original = ToolWorkspaces::new(&original_user, &app_data).unwrap();
    let canonical = ToolWorkspaces::new(&canonical_user, &app_data).unwrap();
    assert_eq!(original.ai_root, canonical.ai_root);
}

#[cfg(windows)]
#[test]
fn links_and_junctions_cannot_be_traversed() {
    use std::os::windows::fs::symlink_dir;
    let fixture = Fixture::new();
    let ws = &fixture.workspaces;
    let external = fixture.root.join("external");
    fs::create_dir(&external).unwrap();
    fs::write(external.join("secret.txt"), "secret").unwrap();
    if symlink_dir(&external, ws.user_root.join("linked")).is_ok() {
        assert!(ws.dispatch("file_read_text", &json!({"space":"user","path":"linked/secret.txt"})).is_err());
        assert!(ws.dispatch("file_export", &json!({"path":"something.txt","user_path":"linked/new.txt"})).is_err());
    }
    if symlink_dir(&external, ws.ai_root.join("linked")).is_ok() {
        assert!(ws.dispatch("file_write_text", &json!({"path":"linked/new.txt","content":"x"})).is_err());
        assert!(ws.check_quota().is_err());
    }
}
