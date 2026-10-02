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
fn workspace_listing_pages_cover_files_beyond_the_legacy_first_hundred() {
    let fixture = Fixture::new();
    let ws = &fixture.workspaces;
    for index in (0..125).rev() {
        fs::write(ws.ai_root.join(format!("item-{index:03}.txt")),format!("{index}")).unwrap();
    }
    let first = ws.dispatch("workspace_list",&json!({"space":"ai"})).unwrap();
    assert_eq!(first["offset"],0);
    assert_eq!(first["limit"],100);
    assert_eq!(first["entries"].as_array().unwrap().len(),100);
    assert_eq!(first["entries"][0]["name"],"item-000.txt");
    assert_eq!(first["entries"][99]["name"],"item-099.txt");
    assert_eq!(first["total"],125);
    assert_eq!(first["next_offset"],100);
    assert_eq!(first["truncated"],true);
    assert_eq!(first["partial"],false);
    assert_eq!(ws.dispatch("workspace_list",&json!({"space":"ai"})).unwrap(),first);
    let final_page = ws.dispatch("workspace_list",&json!({"space":"ai","offset":100})).unwrap();
    assert_eq!(final_page["entries"].as_array().unwrap().len(),25);
    assert_eq!(final_page["entries"][0]["name"],"item-100.txt");
    assert_eq!(final_page["entries"][24]["name"],"item-124.txt");
    assert_eq!(final_page["total"],125);
    assert!(final_page["next_offset"].is_null());
    assert_eq!(final_page["truncated"],false);
    let cross_page = ws.dispatch("workspace_list",&json!({"space":"ai","offset":98,"limit":7})).unwrap();
    assert_eq!(cross_page["entries"].as_array().unwrap().len(),7);
    assert_eq!(cross_page["entries"][0]["name"],"item-098.txt");
    assert_eq!(cross_page["entries"][6]["name"],"item-104.txt");
    assert_eq!(cross_page["next_offset"],105);
}

#[test]
fn workspace_listing_chinese_names_follow_byte_limited_pages_without_missing_files() {
    let fixture = Fixture::new();
    let ws = &fixture.workspaces;
    let expected: Vec<String> = (0..200).map(|index| format!("{index:03}-{}.txt","文件".repeat(35))).collect();
    for name in expected.iter().rev() { fs::write(ws.ai_root.join(name),"data").unwrap(); }
    let mut seen = Vec::new();
    let mut offset = 0usize;
    let mut pages = 0usize;
    loop {
        let args = json!({"space":"ai","offset":offset,"limit":200});
        let page = ws.dispatch("workspace_list",&args).unwrap();
        assert_eq!(page["total"],200);
        assert_eq!(page["partial"],false);
        let rows = page["entries"].as_array().unwrap();
        assert!(!rows.is_empty(),"Byte-limited pages must always make progress");
        assert!(serde_json::to_vec(rows).unwrap().len() <= 16*1024);
        assert_eq!(ws.dispatch("workspace_list",&args).unwrap(),page,"The same byte-limited page is stable");
        seen.extend(rows.iter().map(|entry| entry["name"].as_str().unwrap().to_owned()));
        pages += 1;
        match page["next_offset"].as_u64() {
            Some(next) => {
                assert_eq!(page["page_byte_limited"],true);
                assert_eq!(page["truncated"],true);
                assert_eq!(next as usize,offset+rows.len());
                offset = next as usize;
            },
            None => {
                assert_eq!(page["page_byte_limited"],false);
                assert_eq!(page["truncated"],false);
                break;
            },
        }
        assert!(pages <= expected.len(),"Pagination must not repeat an empty or stale page");
    }
    assert!(pages > 1);
    assert_eq!(seen,expected);
}

#[test]
fn workspace_listing_rejects_invalid_paging_types_fields_and_paths() {
    let fixture = Fixture::new();
    let ws = &fixture.workspaces;
    for bad in [json!(-1),json!(1.5),json!("1"),json!(true),serde_json::Value::Null,json!(10001)] {
        assert!(ws.dispatch("workspace_list",&json!({"space":"user","offset":bad})).is_err());
    }
    for bad in [json!(0),json!(-1),json!(201),json!(1.0),json!("100"),serde_json::Value::Null] {
        assert!(ws.dispatch("workspace_list",&json!({"space":"user","limit":bad})).is_err());
    }
    for path in ["../escape","C:/escape","/escape","a\\b","."] {
        assert!(ws.dispatch("workspace_list",&json!({"space":"user","path":path,"offset":0,"limit":1})).is_err());
    }
    assert!(ws.dispatch("workspace_list",&json!({"space":"user","path":0})).is_err());
    assert!(ws.dispatch("workspace_list",&json!({"space":"user","unknown":1})).is_err());
    assert!(ws.dispatch("workspace_list",&json!({"space":"outside"})).is_err());
    let empty = ws.dispatch("workspace_list",&json!({"space":"user","offset":10000,"limit":200})).unwrap();
    assert_eq!(empty["entries"],json!([]));
    assert_eq!(empty["total"],0);
    assert!(empty["next_offset"].is_null());
    assert_eq!(empty["truncated"],false);
}

#[test]
fn workspace_listing_scan_budgets_remain_explicitly_partial() {
    let fixture = Fixture::new();
    let ws = &fixture.workspaces;
    for name in ["a.txt","b.txt","c.txt"] { fs::write(ws.user_root.join(name),"text").unwrap(); }
    let partial = ws.list_workspace_bounded("user","",0,100,1,
        std::time::Instant::now()+std::time::Duration::from_secs(2)).unwrap();
    assert_eq!(partial["entries"].as_array().unwrap().len(),1);
    assert_eq!(partial["partial"],true);
    assert_eq!(partial["truncated"],true);
    assert!(partial["total"].is_null());
    assert!(!partial["warnings"].as_array().unwrap().is_empty());
    let timed = ws.list_workspace_bounded("user","",0,100,100,std::time::Instant::now()).unwrap();
    assert_eq!(timed["entries"],json!([]));
    assert_eq!(timed["partial"],true);
    assert_eq!(timed["truncated"],true);
    assert!(timed["total"].is_null());
}

#[test]
fn workspace_listing_pages_keep_space_and_independent_file_restrictions() {
    let fixture = Fixture::new();
    let ws = &fixture.workspaces;
    for root in [&ws.user_root,&ws.ai_root] {
        fs::create_dir(root.join("nested")).unwrap();
        fs::write(root.join("nested/normal.txt"),"text").unwrap();
    }
    for space in ["user","ai"] {
        let page = ws.dispatch("workspace_list",&json!({"space":space,"path":"nested","limit":1})).unwrap();
        assert_eq!(page["space"],space);
        assert_eq!(page["path"],"nested");
        assert_eq!(page["entries"][0],json!({"name":"normal.txt","kind":"file","bytes":4}));
        assert_eq!(page["total"],1);
    }
    fs::write(fixture.root.join("external.txt"),"secret").unwrap();
    fs::hard_link(fixture.root.join("external.txt"),ws.ai_root.join("nested/linked.txt")).unwrap();
    assert!(ws.dispatch("workspace_list",&json!({"space":"ai","path":"nested","offset":1,"limit":1})).is_err());
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
    assert!(ws.dispatch("file_export", &json!({"path":"sample.txt","user_path":"missing/deeper/result.txt"})).is_err());
    assert!(!ws.user_root.join("missing").exists());
}

#[test]
fn export_groups_files_in_one_new_folder_without_overwriting() {
    let fixture = Fixture::new();
    let ws = &fixture.workspaces;
    fs::write(ws.ai_root.join("first.wav"), b"first audio").unwrap();
    fs::write(ws.ai_root.join("second.wav"), b"second audio").unwrap();
    fs::write(ws.ai_root.join("probe.wav"), b"not requested").unwrap();
    fs::write(ws.user_root.join("original.wav"), b"original").unwrap();
    for name in ["first.wav", "second.wav"] {
        let path = format!("本次结果/{name}");
        let result = ws.dispatch("file_export", &json!({"path":name,"user_path":path})).unwrap();
        assert_eq!(result["space"], "user");
        assert_eq!(result["path"], path);
        assert_eq!(fs::read(ws.user_root.join(&path)).unwrap(), fs::read(ws.ai_root.join(name)).unwrap());
        assert!(!ws.user_root.join(name).exists());
    }
    assert!(!ws.user_root.join("本次结果/probe.wav").exists());
    assert!(ws.dispatch("file_export", &json!({"path":"second.wav","user_path":"本次结果/first.wav"})).is_err());
    assert_eq!(fs::read(ws.user_root.join("本次结果/first.wav")).unwrap(), b"first audio");
    assert_eq!(fs::read(ws.user_root.join("original.wav")).unwrap(), b"original");
}

#[test]
fn export_folder_creation_is_bounded_and_validates_source_first() {
    let fixture = Fixture::new();
    let ws = &fixture.workspaces;
    assert!(ws.dispatch("file_export", &json!({"path":"absent.wav","user_path":"must-not-create/out.wav"})).is_err());
    assert!(!ws.user_root.join("must-not-create").exists());
    fs::write(ws.ai_root.join("source.wav"), b"audio").unwrap();
    fs::write(ws.user_root.join("occupied"), b"keep").unwrap();
    for path in ["occupied/out.wav", "../escape/out.wav", "new/deep/out.wav"] {
        assert!(ws.dispatch("file_export", &json!({"path":"source.wav","user_path":path})).is_err());
    }
    assert_eq!(fs::read(ws.user_root.join("occupied")).unwrap(), b"keep");
    assert!(!ws.user_root.join("new").exists());
    fs::create_dir_all(ws.user_root.join("existing/deep")).unwrap();
    ws.dispatch("file_export", &json!({"path":"source.wav","user_path":"existing/deep/out.wav"})).unwrap();
    assert_eq!(fs::read(ws.user_root.join("existing/deep/out.wav")).unwrap(), b"audio");
}

#[test]
fn ai_directory_creation_is_idempotent_and_three_levels_allow_file_operations() {
    let fixture = Fixture::new();
    let ws = &fixture.workspaces;
    let created = ws.dispatch("directory_create", &json!({"path":"project/round/output"})).unwrap();
    assert_eq!(created["space"], "ai");
    assert_eq!(created["created"], true);
    assert_eq!(created["max_depth"], 3);
    assert!(ws.ai_root.join("project/round/output").is_dir());
    assert!(!ws.user_root.join("project").exists());
    fs::write(ws.ai_root.join("occupied"), b"keep file").unwrap();
    assert!(ws.dispatch("directory_create", &json!({"path":"occupied"})).is_err());
    assert_eq!(fs::read(ws.ai_root.join("occupied")).unwrap(), b"keep file");
    assert_eq!(ws.dispatch("directory_create", &json!({"path":"project/round/output"})).unwrap()["created"], false);
    ws.dispatch("file_write_text", &json!({"path":"project/round/output/note.txt","content":"kept"})).unwrap();
    fs::write(ws.user_root.join("source.txt"), b"copy").unwrap();
    ws.dispatch("file_copy_to_ai", &json!({"source_space":"user","source_path":"source.txt","path":"copy/round/output/copied.txt"})).unwrap();
    assert_eq!(fs::read(ws.ai_root.join("copy/round/output/copied.txt")).unwrap(), b"copy");
    assert!(ws.dispatch("directory_create", &json!({"path":"project/round/output/note.txt"})).is_err());
    assert_eq!(fs::read(ws.ai_root.join("project/round/output/note.txt")).unwrap(), b"kept");
}

#[test]
fn ai_directory_depth_cannot_be_bypassed_through_write_or_copy() {
    let fixture = Fixture::new();
    let ws = &fixture.workspaces;
    fs::write(ws.user_root.join("source.txt"), b"copy").unwrap();
    for path in ["", ".", "../escape", "C:/outside", "new/two/three/four"] {
        assert!(ws.dispatch("directory_create", &json!({"path":path})).is_err(), "accepted {path}");
    }
    assert!(ws.dispatch("directory_create", &json!({"space":"user","path":"forbidden"})).is_err());
    assert!(ws.dispatch("file_write_text", &json!({"path":"write/two/three/four/file.txt","content":"x"})).is_err());
    assert!(ws.dispatch("file_copy_to_ai", &json!({"source_space":"user","source_path":"source.txt","path":"copy/two/three/four/file.txt"})).is_err());
    for path in ["new","write","copy","forbidden"] { assert!(!ws.ai_root.join(path).exists()); }
    assert!(!ws.user_root.join("forbidden").exists());
    fs::create_dir_all(ws.ai_root.join("old/two/three/four")).unwrap();
    assert!(ws.dispatch("directory_create", &json!({"path":"old/two/three/four"})).is_err());
    assert!(ws.dispatch("file_write_text", &json!({"path":"old/two/three/four/file.txt","content":"x"})).is_err());
    assert!(!ws.ai_root.join("old/two/three/four/file.txt").exists());
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
    fs::write(ws.ai_root.join("something.txt"), b"valid export source").unwrap();
    if symlink_dir(&external, ws.user_root.join("linked")).is_ok() {
        assert!(ws.dispatch("file_read_text", &json!({"space":"user","path":"linked/secret.txt"})).is_err());
        assert!(ws.dispatch("file_export", &json!({"path":"something.txt","user_path":"linked/new.txt"})).is_err());
        assert!(!external.join("new.txt").exists());
    }
    if symlink_dir(&external, ws.ai_root.join("linked")).is_ok() {
        assert!(ws.dispatch("directory_create", &json!({"path":"linked/folder"})).is_err());
        assert!(!external.join("folder").exists());
        assert!(ws.dispatch("file_write_text", &json!({"path":"linked/new.txt","content":"x"})).is_err());
        assert!(ws.check_quota().is_err());
    }
}
