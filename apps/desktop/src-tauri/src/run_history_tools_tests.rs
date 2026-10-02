use super::*;
use crate::run_records::{RunDraft, RunFileDraft};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture { root: PathBuf, app: PathBuf, spaces: ToolWorkspaces, store: RunStore }

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("audioprocess-history-tools-{}-{nonce}-{}",
            std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let user = root.join("user"); let app = root.join("app");
        fs::create_dir_all(&user).unwrap(); fs::create_dir(&app).unwrap();
        let spaces = ToolWorkspaces::new(&user, &app).unwrap();
        Self { root, app, spaces, store: RunStore::default() }
    }

    fn create(&self, kind: &str, parent: Option<&str>, config: Value, files: Vec<RunFileDraft>) -> RunRecord {
        self.store.begin(&self.spaces, &self.app, RunDraft { kind:kind.into(),origin:"manual".into(),
            parent_id:parent.map(str::to_owned),name:"History test".into(),configuration:config,files }).unwrap()
    }

    fn call(&self, tool: &str, args: Value) -> Value {
        let reply = dispatch(&self.store, &self.spaces, &self.app, tool, &args).unwrap();
        assert!(bytes(&reply) <= REPLY_BYTES);
        reply
    }

    fn record_path(&self, id: &str) -> PathBuf {
        let identity = if cfg!(windows) { self.spaces.user_root.to_string_lossy().to_lowercase() }
            else { self.spaces.user_root.to_string_lossy().into_owned() };
        self.app.join("run-records").join("v1").join(format!("{:x}", Sha256::digest(identity.as_bytes())))
            .join("records").join(format!("{id}.json"))
    }
}

impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.root); } }

fn file(path: &str) -> RunFileDraft { RunFileDraft { space:"user".into(),path:path.into(),role:"input".into() } }

#[test]
fn schemas_are_read_only_and_reject_free_paths() {
    assert_eq!(definitions().len(), NAMES.len());
    let fixture = Fixture::new();
    for name in NAMES {
        let definition = definitions().into_iter().find(|d| d["function"]["name"] == *name).unwrap();
        assert_eq!(definition["function"]["parameters"]["additionalProperties"], false);
        assert!(dispatch(&fixture.store, &fixture.spaces, &fixture.app, name,
            &json!({"workspace":"C:/other","id":"a".repeat(64)})).is_err());
        assert!(dispatch(&fixture.store, &fixture.spaces, &fixture.app, name,
            &json!({"root":"../other","id":"a".repeat(64)})).is_err());
    }
}

#[test]
fn filters_and_keyset_cursor_match_store_ties_without_duplicates() {
    let fixture = Fixture::new();
    let parent = fixture.create("workflow", None, json!({}), vec![]);
    let a = fixture.create("graph", Some(&parent.id), json!({}), vec![]);
    let b = fixture.create("graph", Some(&parent.id), json!({}), vec![]);
    let failed = fixture.create("graph", None, json!({}), vec![]);
    fixture.store.finish(&fixture.spaces, &fixture.app, &failed.id, "failed", None, Some("failure".into()), vec![]).unwrap();
    // Force tied timestamps in the uniquely owned fixture to verify RunStore's descending ID tie-break.
    for record in [&a, &b] {
        let mut value = serde_json::to_value(record).unwrap(); value["started_at_ms"] = json!(500);
        fs::write(fixture.record_path(&record.id), serde_json::to_vec(&value).unwrap()).unwrap();
    }
    let first = fixture.call("runs_list", json!({"kind":"graph","state":"running","parent_id":parent.id,"limit":1}));
    fixture.create("graph", Some(&parent.id), json!({"newer":true}), vec![]);
    let second = fixture.call("runs_list", json!({"kind":"graph","state":"running","parent_id":parent.id,"limit":1,"cursor":first["next_cursor"]}));
    assert_eq!(first["records"][0]["id"], json!(std::cmp::max(&a.id, &b.id)));
    assert_eq!(second["records"][0]["id"], json!(std::cmp::min(&a.id, &b.id)));
    assert!(second["next_cursor"].is_null());
    assert_eq!(second["history_incomplete"], false);
    let failed_only = fixture.call("runs_list", json!({"state":"failed","parent_id":null}));
    assert_eq!(failed_only["records"].as_array().unwrap().len(), 1);
    assert_eq!(failed_only["records"][0]["id"], failed.id);
    assert!(dispatch(&fixture.store,&fixture.spaces,&fixture.app,"runs_list",&json!({"limit":21})).is_err());
}

#[test]
fn malformed_source_cannot_make_empty_matches_claim_complete_history() {
    let fixture = Fixture::new();
    let run = fixture.create("graph", None, json!({}), vec![]);
    fs::write(fixture.record_path(&"b".repeat(64)), b"not-json").unwrap();
    let page = fixture.call("runs_list", json!({"kind":"workflow"}));
    assert!(page["records"].as_array().unwrap().is_empty());
    assert_eq!(page["history_incomplete"], true);
    assert_eq!(page["complete"], false);
    assert!(!page["warnings"].as_array().unwrap().is_empty());
    assert!(fixture.record_path(&run.id).is_file());
}

#[test]
fn store_list_cap_remains_explicit_after_filtering_and_last_cursor() {
    let fixture = Fixture::new();
    let run = fixture.create("graph",None,json!({}),vec![]);
    let mut value = serde_json::to_value(&run).unwrap();
    for index in 0..501u64 {
        let id = format!("{index:064x}");
        value["id"] = json!(id); value["started_at_ms"] = json!(index + 1);
        fs::write(fixture.record_path(&id),serde_json::to_vec(&value).unwrap()).unwrap();
    }
    let page = fixture.call("runs_list",json!({"kind":"workflow"}));
    assert_eq!(page["history_incomplete"],true); assert_eq!(page["complete"],false);
    assert!(page["records"].as_array().unwrap().is_empty());
    assert!(page["next_cursor"].is_null());
}

#[test]
fn oversized_configuration_is_directory_and_children_are_read_on_demand() {
    let fixture = Fixture::new();
    let run = fixture.create("graph", None, json!({"graph":{"nodes":[{"name":"small"}],"text":"x".repeat(50_000)},"options":{"rate":48000}}), vec![]);
    let root = fixture.call("runs_read", json!({"id":run.id,"section":"configuration"}));
    assert_eq!(root["view"], "directory"); assert_eq!(root["complete"], false);
    assert!(root.get("data").is_none());
    let child = fixture.call("runs_read", json!({"id":run.id,"section":"configuration","pointer":"/options"}));
    assert_eq!(child["view"], "inline"); assert_eq!(child["complete"], true);
    assert_eq!(child["data"], json!({"rate":48000}));
    let paged = fixture.call("runs_read", json!({"id":run.id,"section":"configuration","limit":1}));
    assert_eq!(paged["entries"].as_array().unwrap().len(), 1); assert_eq!(paged["next_offset"], 1);
    let last = fixture.call("runs_read", json!({"id":run.id,"section":"configuration","offset":1,"limit":1}));
    assert!(last["next_offset"].is_null()); assert_eq!(last["complete"], false);
}

#[test]
fn pointers_unescape_slash_tilde_and_reject_invalid_escapes() {
    let fixture = Fixture::new();
    let run = fixture.create("graph", None, json!({"a/b":{"~key":["zero",{"":"empty key"}]}}), vec![]);
    let page = fixture.call("runs_read", json!({"id":run.id,"section":"configuration","pointer":"/a~1b/~0key/1/"}));
    assert_eq!(page["data"], "empty key");
    for pointer in ["/a~2b", "/~", "a", "/a~1b/~0key/01", "/missing"] {
        assert!(dispatch(&fixture.store,&fixture.spaces,&fixture.app,"runs_read",
            &json!({"id":run.id,"section":"configuration","pointer":pointer})).is_err(), "accepted {pointer}");
    }
}

#[test]
fn long_unicode_text_and_escaped_values_are_bounded_explicit_slices() {
    let fixture = Fixture::new();
    let text = "中\0\n".repeat(20_000);
    let run = fixture.create("graph", None, json!({"text":text}), vec![]);
    let page = fixture.call("runs_read", json!({"id":run.id,"section":"configuration","pointer":"/text","offset":2,"limit":3}));
    assert_eq!(page["view"], "text"); assert_eq!(page["data"], "\n中\0");
    assert_eq!(page["total_chars"], 60_000); assert_eq!(page["next_offset"], 5); assert_eq!(page["complete"], false);
    let default = fixture.call("runs_read", json!({"id":run.id,"section":"configuration","pointer":"/text"}));
    assert_eq!(default["next_offset"], 2048); assert_eq!(default["complete"], false);
    let long_key = "\0".repeat(20_000);
    let keys = fixture.create("graph", None, json!({long_key:42}), vec![]);
    let directory = fixture.call("runs_read", json!({"id":keys.id,"section":"configuration"}));
    assert_eq!(directory["entries"][0]["pointer_omitted"], true);
    assert_eq!(directory["entries"][0]["read_available"], false);
    assert!(directory["next_offset"].is_null());
    let escaped_key = "\0".repeat(4000);
    let keys = fixture.create("graph", None, json!({escaped_key:42}), vec![]);
    let directory = fixture.call("runs_read", json!({"id":keys.id,"section":"configuration"}));
    assert_eq!(directory["entries"].as_array().unwrap().len(),1);
    assert_eq!(directory["entries"][0]["read_available"],false);
    assert!(directory["next_offset"].is_null());
}

#[test]
fn current_project_only_and_traversal_ids_are_rejected_without_mutation() {
    let fixture = Fixture::new(); let other = Fixture::new();
    fs::write(fixture.spaces.user_root.join("input.wav"), b"audio").unwrap();
    let run = fixture.create("graph", None, json!({"nodes":[]}), vec![file("input.wav")]);
    let before = fs::read(fixture.record_path(&run.id)).unwrap();
    let input_before = fs::read(fixture.spaces.user_root.join("input.wav")).unwrap();
    for tool in ["runs_read", "runs_check_files"] {
        assert!(dispatch(&other.store,&other.spaces,&other.app,tool,&json!({"id":run.id})).is_err());
        for id in ["../outside", "C:/outside", "/absolute", "A".repeat(64).as_str()] {
            assert!(dispatch(&fixture.store,&fixture.spaces,&fixture.app,tool,&json!({"id":id})).is_err());
        }
        fixture.call(tool, json!({"id":run.id}));
    }
    fixture.call("runs_list", json!({}));
    assert_eq!(fs::read(fixture.record_path(&run.id)).unwrap(), before);
    assert_eq!(fs::read(fixture.spaces.user_root.join("input.wav")).unwrap(), input_before);
}

#[test]
fn file_checks_preserve_hash_semantics_and_page_statuses() {
    let fixture = Fixture::new(); let input = fixture.spaces.user_root.join("input.wav");
    fs::write(&input, b"audio A").unwrap();
    let run = fixture.create("graph",None,json!({}),vec![file("input.wav"),file("later.wav")]);
    fs::write(fixture.spaces.user_root.join("later.wav"),b"later").unwrap();
    let available = fixture.call("runs_check_files",json!({"id":run.id,"limit":1}));
    assert_eq!(available["files"][0]["status"],"available"); assert_eq!(available["next_offset"],1);
    assert_eq!(available["not_backup"],true); assert!(available["checked_at_ms"].as_u64().unwrap()>0);
    let second = fixture.call("runs_check_files",json!({"id":run.id,"offset":1,"limit":1}));
    assert_eq!(second["files"][0]["status"],"unverified");
    fs::write(&input,b"audio B").unwrap();
    assert_eq!(fixture.call("runs_check_files",json!({"id":run.id}))["files"][0]["status"],"changed");
    fs::remove_file(&input).unwrap();
    assert_eq!(fixture.call("runs_check_files",json!({"id":run.id}))["files"][0]["status"],"missing");
}

#[test]
fn succeeded_record_summary_keeps_error_and_omitted_result_warning() {
    let fixture = Fixture::new(); let run = fixture.create("graph",None,json!({}),vec![]);
    fixture.store.finish(&fixture.spaces,&fixture.app,&run.id,"succeeded",Some(json!("x".repeat(600*1024))),
        Some("cleanup failed".into()),vec![]).unwrap();
    let read = fixture.call("runs_read",json!({"id":run.id}));
    assert_eq!(read["data"]["state"],"succeeded"); assert_eq!(read["data"]["error"],"cleanup failed");
    assert!(read["data"]["recording_warning"].as_str().unwrap().contains("omitted"));
    let result = fixture.call("runs_read",json!({"id":run.id,"section":"result"}));
    assert!(result["data"].is_null());
    assert!(result["recording_warning"].as_str().unwrap().contains("omitted"));
}

#[test]
fn hostile_summary_text_stays_inline_with_explicit_truncation() {
    let fixture = Fixture::new(); let run = fixture.create("graph",None,json!({}),vec![]);
    let mut value = serde_json::to_value(&run).unwrap();
    value["name"] = json!("\0".repeat(4000));
    value["error"] = json!("\0".repeat(4000));
    value["recording_warning"] = json!("\0".repeat(4000));
    fs::write(fixture.record_path(&run.id),serde_json::to_vec(&value).unwrap()).unwrap();
    let read = fixture.call("runs_read",json!({"id":run.id,"limit":1}));
    assert_eq!(read["view"],"inline"); assert_eq!(read["complete"],false);
    assert_eq!(read["data"]["name_truncated"],true);
    assert_eq!(read["data"]["error_truncated"],true);
    assert_eq!(read["data"]["recording_warning_truncated"],true);
    assert!(bytes(&read["data"])<=INLINE_BYTES);
    let list = fixture.call("runs_list",json!({}));
    assert_eq!(list["complete"],false); assert_eq!(list["records"][0]["name_truncated"],true);
}
