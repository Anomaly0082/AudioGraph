use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn spaces() -> (std::path::PathBuf, ToolWorkspaces) {
    let root = std::env::temp_dir().join(format!("audioprocess-workflow-editor-{}-{}",
        std::process::id(), NEXT.fetch_add(1,Ordering::Relaxed)));
    std::fs::create_dir_all(root.join("user")).unwrap();
    let spaces = ToolWorkspaces::new(&root.join("user"),&root.join("data")).unwrap();
    (root,spaces)
}

fn program() -> String {
    json!({"schema_version":1,"inputs":{},"steps":[
        {"id":"write","type":"call","tool":"file_write_text","args":{"path":"result.txt","content":"snapshot"}},
        {"id":"export","type":"call","tool":"file_export","args":{"path":"result.txt","user_path":"deliver/result.txt"}}
    ],"outputs":{"receipt":{"$ref":"/steps/export"}}}).to_string()
}

#[test]
fn validation_is_strict_bounded_and_pure() {
    let (root,spaces) = spaces();
    let text = program();
    assert_eq!(validate_text(&text)["valid"],true);
    assert!(!spaces.ai_root.join("result.txt").exists());
    assert!(!spaces.user_root.join("deliver").exists());
    assert!(!root.join("data/run-records").exists());
    for invalid in [
        "{\"schema_version\":1,\"schema_version\":1,\"inputs\":{},\"steps\":[],\"outputs\":{}}".to_owned(),
        format!("{text} trailing"),
        format!("{text}{}"," ".repeat(MAX_TEXT)),
        "{\"schema_version\":1.0,\"inputs\":{},\"steps\":[],\"outputs\":{}}".to_owned(),
    ] { assert_eq!(validate_text(&invalid)["valid"],false); }
    let bad = json!({"schema_version":1,"inputs":{},"steps":[{"id":"run","type":"call","tool":"workflow_run","args":{}}],"outputs":{}});
    assert_eq!(validate_text(&bad.to_string())["valid"],false);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn load_allows_invalid_json_and_rejects_unsafe_or_non_text_files() {
    let (root,spaces) = spaces();
    std::fs::write(spaces.user_root.join("invalid.workflow.json"),"{ broken").unwrap();
    assert_eq!(load_text(&spaces,"user","invalid.workflow.json").unwrap()["text"],"{ broken");
    std::fs::write(spaces.ai_root.join("binary.json"),[0xff]).unwrap();
    std::fs::write(spaces.ai_root.join("oversize.json"),vec![b' ';MAX_TEXT+1]).unwrap();
    for (space,path) in [("ai","binary.json"),("ai","oversize.json"),("user","../invalid.workflow.json"),
        ("user","C:/outside.json"),("user","a\\b.json"),("other","invalid.workflow.json")] {
        assert!(load_text(&spaces,space,path).is_err(),"{space} {path}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn save_validates_before_writing_and_never_overwrites_in_either_space() {
    let (root,spaces) = spaces();
    assert!(save_text(&spaces,"user","invalid/new.json","{").is_err());
    assert!(!spaces.user_root.join("invalid").exists());
    let text = program();
    for space in ["user","ai"] {
        let saved = save_text(&spaces,space,"saved/config.workflow.json",&text).unwrap();
        assert_eq!(saved,json!({"space":space,"path":"saved/config.workflow.json"}));
        assert!(save_text(&spaces,space,"saved/config.workflow.json",&text.replace("snapshot","changed")).is_err());
        assert_eq!(load_text(&spaces,space,"saved/config.workflow.json").unwrap()["text"],text);
    }
    assert!(save_text(&spaces,"ai","a/b/c/d/config.json",&text).is_err());
    assert!(!spaces.ai_root.join("a").exists());
    assert!(save_text(&spaces,"user","a/b/config.json",&text).is_err());
    assert!(!spaces.user_root.join("a").exists());
    for path in ["../escape.json","/absolute.json","C:/escape.json","a\\b.json","CON.json",""] {
        assert!(save_text(&spaces,"ai",path,&text).is_err(),"{path}");
    }
    save_text(&spaces,"ai","a/b/c/config.json",&text).unwrap();
    std::fs::create_dir_all(spaces.user_root.join("existing/nested")).unwrap();
    save_text(&spaces,"user","existing/nested/config.json",&text).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn inline_manual_snapshot_records_outputs_and_raw_report_without_model_or_saved_path() {
    tauri::async_runtime::block_on(async {
        let (root,spaces) = spaces();
        let store = Arc::new(RunStore::default());
        let data = root.join("data");
        let manager = AgentManager::default();
        let (cancel,lease) = manager.begin_manual("inline-run").unwrap();
        let mut tools = ToolContext::new(spaces,Arc::new(BackendManager::default()),"unconnected".into())
            .with_records(store.clone(),data.clone()).with_manual_origin().with_failure_sink(manager.failure_sink());
        let text = program();
        let report = run_snapshot(&mut tools,&text,&cancel).await.unwrap();
        assert_eq!(report["state"],"succeeded","{report}");
        assert_eq!(report["source"],json!({"kind":"editor","sha256":format!("{:x}",Sha256::digest(text.as_bytes()))}));
        let record = store.load(&tools.spaces,&data,report["run_id"].as_str().unwrap()).unwrap();
        assert_eq!(record.origin,"manual");
        assert_eq!(record.kind,"workflow");
        assert_eq!(record.configuration["file_space"],"ai");
        assert_eq!(record.configuration["workflow"],serde_json::from_str::<Value>(&text).unwrap());
        assert_eq!(record.result,Some(report));
        assert!(record.files.iter().all(|file| file.role != "input"));
        assert!(record.files.iter().any(|file| file.space == "user" && file.path == "deliver/result.txt" && file.sha256.is_some()));
        assert_eq!(std::fs::read_to_string(tools.spaces.ai_root.join("result.txt")).unwrap(),"snapshot");
        assert_eq!(std::fs::read_to_string(tools.spaces.user_root.join("deliver/result.txt")).unwrap(),"snapshot");
        // Repeating the immutable snapshot fails on the existing export and preserves the receipt.
        let repeated = run_snapshot(&mut tools,&text,&cancel).await.unwrap();
        assert_eq!(repeated["state"],"failed");
        assert_eq!(store.load(&tools.spaces,&data,repeated["run_id"].as_str().unwrap()).unwrap().state,"failed");
        drop(tools);
        drop(lease);
        std::fs::remove_dir_all(root).unwrap();
    });
}

#[test]
fn manual_cancel_and_close_share_agent_lease_and_record_cancelled_state() {
    tauri::async_runtime::block_on(async {
        let (root,spaces) = spaces();
        let store = Arc::new(RunStore::default());
        let data = root.join("data");
        let manager = AgentManager::default();
        let (cancel,lease) = manager.begin_manual("stop-me").unwrap();
        assert!(manager.begin_manual("ai-or-manual").is_err());
        assert!(manager.with_idle(|| Ok(())).is_err());
        assert!(manager.cancel("stop-me"));
        let mut tools = ToolContext::new(spaces,Arc::new(BackendManager::default()),"unconnected".into())
            .with_records(store.clone(),data.clone()).with_manual_origin();
        let report = run_snapshot(&mut tools,&program(),&cancel).await.unwrap();
        assert_eq!(report["state"],"cancelled");
        assert!(!tools.spaces.ai_root.join("result.txt").exists());
        assert_eq!(store.load(&tools.spaces,&data,report["run_id"].as_str().unwrap()).unwrap().state,"cancelled");
        manager.shutdown();
        assert!(manager.wait_idle(std::time::Duration::ZERO).is_err());
        drop(tools);
        drop(lease);
        manager.wait_idle(std::time::Duration::from_millis(1)).unwrap();
        assert!(manager.begin_manual("after-close").is_err());
        manager.reopen();
        assert!(manager.begin_manual("after-close").is_ok());
        std::fs::remove_dir_all(root).unwrap();
    });
}
