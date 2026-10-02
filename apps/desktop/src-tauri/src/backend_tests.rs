use super::*;
use crate::graph_files::{load_graph_file, save_graph_file, strict_json};
use std::sync::atomic::AtomicUsize;

fn isolated_shared() -> (Arc<Shared>, Arc<Mutex<Vec<DisconnectedEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let (writer, _receiver) = mpsc::sync_channel(MAX_PENDING);
    let shared = Arc::new(Shared {
        session_id: "unit-session".into(),
        alive: AtomicBool::new(true),
        pending: Mutex::new(HashMap::new()),
        writer: Mutex::new(Some(writer)),
        reason: Mutex::new(String::new()),
        stderr: Mutex::new(VecDeque::new()),
        // 无子进程的fixture预设退出结果，Connection析构无需等待monitor。
        exited: Mutex::new(Some(DisconnectReport { forced: false, message: "mock exited".into() })),
        exit_ready: Condvar::new(),
        on_disconnect: Arc::new(move |event| captured.lock().unwrap().push(event)),
    });
    (shared, events)
}

#[test]
fn replies_are_correlated_out_of_order_and_unknown_ids_are_ignored() {
    let (shared, _) = isolated_shared();
    let (first_sender, first) = mpsc::channel();
    let (second_sender, second) = mpsc::channel();
    shared.pending.lock().unwrap().insert("request-1".into(), first_sender);
    shared.pending.lock().unwrap().insert("request-2".into(), second_sender);
    dispatch_response(&shared, br#"{"schema_version":1,"id":"expired","success":true}"#).unwrap();
    assert!(matches!(first.try_recv(), Err(mpsc::TryRecvError::Empty)));
    dispatch_response(&shared, br#"{"schema_version":1,"id":"request-2","success":true,"data":2}"#).unwrap();
    assert_eq!(second.recv_timeout(Duration::from_secs(1)).unwrap().unwrap()["data"], 2);
    assert!(matches!(first.try_recv(), Err(mpsc::TryRecvError::Empty)));
    dispatch_response(&shared, br#"{"schema_version":1,"id":"request-1","success":false,"errors":[]}"#).unwrap();
    assert_eq!(first.recv_timeout(Duration::from_secs(1)).unwrap().unwrap()["success"], false);
    assert!(shared.pending.lock().unwrap().is_empty());
}

#[test]
fn invalid_responses_do_not_consume_pending_requests() {
    let (shared, _) = isolated_shared();
    let (sender, receiver) = mpsc::channel();
    shared.pending.lock().unwrap().insert("request".into(), sender);
    for message in [
        b"not json".as_slice(),
        br#"{"schema_version":2,"id":"request","success":true}"#,
        br#"{"schema_version":1,"id":"request","success":"yes"}"#,
        br#"{"schema_version":1,"id":null,"success":true}"#,
    ] {
        assert!(dispatch_response(&shared, message).is_err());
    }
    assert_eq!(shared.pending.lock().unwrap().len(), 1);
    assert!(matches!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty)));
    shared.close("invalid response");
    assert!(receiver.recv_timeout(Duration::from_secs(1)).unwrap().is_err());
}

#[test]
fn related_managers_share_one_immutable_plugin_selection_and_initialization_error() {
    let empty = BackendManager::default();
    let related = empty.related_manager();
    assert!(Arc::ptr_eq(&empty.plugin_snapshot().unwrap(),&related.plugin_snapshot().unwrap()));
    assert!(empty.plugin_snapshot().unwrap().launch_for(Path::new("unused-default-workspace")).unwrap().is_none());
    let failed = BackendManager::default();
    assert!(failed.configure_plugin_snapshot(Err("test metadata capture failed".into())).is_err());
    assert!(failed.related_manager().plugin_snapshot().unwrap_err().contains("test metadata capture failed"));
    assert!(failed.configure_plugin_snapshot(Ok(Arc::new(PluginSnapshot::default()))).is_err());
    assert!(failed.connect("unused".into(),false,false,Arc::new(|_| {})).err().unwrap().contains("test metadata capture failed"));
}

#[test]
fn plugin_launch_arguments_are_fixed_snapshot_inputs_and_reconnections_do_not_rescan() {
    let fixture = TempDirectory::new();
    let packages = fixture.0.join("config/plugins");
    let first_package = packages.join("first");
    std::fs::create_dir_all(&first_package).unwrap();
    std::fs::write(first_package.join("manifest.json"),b"{}").unwrap();
    let snapshot = Arc::new(PluginSnapshot::capture(&packages,&fixture.0.join("data")).unwrap());
    let manager = BackendManager::default();
    manager.configure_plugin_snapshot(Ok(snapshot.clone())).unwrap();
    let related = manager.related_manager();
    assert!(Arc::ptr_eq(&snapshot,&related.plugin_snapshot().unwrap()));
    let arguments = |plugins: &PluginSnapshot,workspace: &Path| {
        Connection::launch_command(Path::new("fixed-sidecar"),workspace,false,false,plugins).unwrap()
            .get_args().map(|value| value.to_string_lossy().into_owned()).collect::<Vec<_>>()
    };
    let first = arguments(&snapshot,&fixture.0);
    assert_eq!(first[0],"--workspace");
    assert_eq!(first[2],"--plugin-snapshot");
    assert_eq!(first[4],"--plugin-snapshot-sha256");
    assert_eq!(first[6],"--plugin-data");
    assert_eq!(first[5].len(),64);
    std::fs::create_dir(packages.join("added-later")).unwrap();
    std::fs::write(packages.join("added-later/manifest.json"),b"{}").unwrap();
    let reopened = arguments(&manager.plugin_snapshot().unwrap(),&fixture.0);
    let ai = arguments(&related.plugin_snapshot().unwrap(),&fixture.0);
    assert_eq!(first,reopened);
    assert_eq!(first,ai);
    let captured: Value = serde_json::from_slice(&std::fs::read(&first[3]).unwrap()).unwrap();
    assert_eq!(captured["packages"].as_array().unwrap().len(),1);
    let default_args = arguments(&PluginSnapshot::default(),&fixture.0);
    assert_eq!(default_args.len(),2);
}

#[test]
fn connection_close_rejects_every_waiter_once_and_invalidates_writer() {
    let (shared, events) = isolated_shared();
    let mut receivers = Vec::new();
    for index in 0..MAX_PENDING {
        let (sender, receiver) = mpsc::channel();
        shared.pending.lock().unwrap().insert(format!("request-{index}"), sender);
        receivers.push(receiver);
    }
    shared.close("pipe closed");
    shared.close("must not replace original close");
    for receiver in receivers {
        assert_eq!(receiver.recv_timeout(Duration::from_secs(1)).unwrap().unwrap_err(), "pipe closed");
    }
    assert!(!shared.alive.load(Ordering::Acquire));
    assert!(shared.pending.lock().unwrap().is_empty());
    assert!(shared.writer.lock().unwrap().is_none());
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].session_id, "unit-session");
}

#[test]
fn request_limits_and_allowlist_are_checked_without_queueing_work() {
    let (shared, _) = isolated_shared();
    let connection = Connection {
        shared: shared.clone(), workspace: PathBuf::new(), next_id: AtomicU64::new(1),
        threads: Mutex::new(Vec::new()),
    };
    assert!(connection.request(json!(["not an object"])).is_err());
    assert!(connection.request(json!({"op":"shell.exec", "command":"never run"})).is_err());
    assert!(connection.request(json!({"op":"nodes.list", "padding":"x".repeat(MAX_REQUEST)})).is_err());
    assert!(shared.pending.lock().unwrap().is_empty());
    let mut receivers = Vec::new();
    for index in 0..MAX_PENDING {
        let (sender, receiver) = mpsc::channel();
        shared.pending.lock().unwrap().insert(format!("existing-{index}"), sender);
        receivers.push(receiver);
    }
    assert!(connection.request(json!({"op":"nodes.list"})).is_err());
    assert_eq!(shared.pending.lock().unwrap().len(), MAX_PENDING);
    shared.close("end test");
    assert!(connection.request(json!({"op":"nodes.list"})).is_err());
    drop(receivers);
}

struct TempDirectory(PathBuf);
impl TempDirectory {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(1);
        for _ in 0..100 {
            let candidate = std::env::temp_dir().join(format!("audioprocess_desktop_{}_{}",
                std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            match std::fs::create_dir(&candidate) {
                Ok(()) => return Self(std::fs::canonicalize(candidate).unwrap()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("Cannot create isolated test directory: {error}"),
            }
        }
        panic!("Cannot reserve isolated test directory");
    }
}
impl Drop for TempDirectory {
    fn drop(&mut self) {
        // 仅清理由本fixture原子创建的目录；测试不接受外部提供的删除路径。
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn text_graph() -> Value {
    json!({"schema_version":1,"nodes":[{"id":"text","type":"text_input",
        "parameters":{"text":"你好，任务接口"}}],"connections":[],
        "exports":[{"name":"message","node":"text","port":"text"}]})
}

#[test]
fn strict_graph_json_rejects_duplicates_trailing_data_and_limits() {
    for data in [
        br#"{"schema_version":1,"schema_version":2}"#.as_slice(),
        br#"{"a":1,"\u0061":2}"#,
        br#"{"nested":{"x":1,"x":2}}"#,
        br#"{} {}"#,
        br#"[]"#,
        &[b'{', b'"', 0xff, b'"', b':', b'1', b'}'],
    ] {
        assert!(strict_json(data).is_err());
    }
    let mut nested = "0".to_owned();
    for _ in 0..70 { nested = format!("[{nested}]"); }
    assert!(strict_json(format!("{{\"nested\":{nested}}}").as_bytes()).is_err());
    assert!(strict_json(&vec![b' '; MAX_REQUEST + 1]).is_err());
    assert_eq!(strict_json(&serde_json::to_vec(&text_graph()).unwrap()).unwrap(), text_graph());
}

#[test]
fn graph_load_save_stays_in_workspace_and_never_overwrites() {
    let fixture = TempDirectory::new();
    let workspace = fixture.0.join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let workspace = std::fs::canonicalize(workspace).unwrap();
    let graph = text_graph();
    save_graph_file(&workspace, "中文.json", &graph).unwrap();
    let loaded = load_graph_file(&workspace, "中文.json").unwrap();
    assert_eq!(loaded.graph, graph);
    assert!(save_graph_file(&workspace, "中文.json", &json!({"schema_version":1})).is_err());
    assert_eq!(load_graph_file(&workspace, "中文.json").unwrap().graph, graph);
    assert!(save_graph_file(&workspace, "../outside.json", &graph).is_err());
    assert!(!fixture.0.join("outside.json").exists());
    std::fs::write(fixture.0.join("outside.json"), serde_json::to_vec(&graph).unwrap()).unwrap();
    assert!(load_graph_file(&workspace, "../outside.json").is_err());
    assert!(save_graph_file(&workspace, "wrong.txt", &graph).is_err());
    assert!(save_graph_file(&workspace, "missing/new.json", &graph).is_err());
    std::fs::write(workspace.join("float-version.json"), br#"{"schema_version":1.0}"#).unwrap();
    assert!(load_graph_file(&workspace, "float-version.json").is_err());
    std::fs::write(workspace.join("duplicate.json"), br#"{"schema_version":1,"schema_version":2}"#).unwrap();
    assert!(load_graph_file(&workspace, "duplicate.json").is_err());
}

#[cfg(windows)]
fn control_executable() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for candidate in [root.join("build/Debug/control-cli.exe"), root.join("build/Release/control-cli.exe")] {
        if candidate.is_file() { return std::fs::canonicalize(candidate).unwrap(); }
    }
    panic!("Build the C++ control-cli target before running the desktop backend integration test");
}

#[cfg(windows)]
#[test]
fn real_sidecar_reuses_captured_packages_on_reconnect_and_keeps_builtin_nodes_on_package_errors() {
    let fixture = TempDirectory::new();
    let packages = fixture.0.join("config/plugins");
    std::fs::create_dir_all(packages.join("invalid-first")).unwrap();
    std::fs::write(packages.join("invalid-first/manifest.json"),b"{}").unwrap();
    let snapshot = Arc::new(PluginSnapshot::capture(&packages,&fixture.0.join("data")).unwrap());
    let executable = control_executable();
    let callback: DisconnectCallback = Arc::new(|_| {});
    let first = Connection::spawn_with_plugins(&executable,fixture.0.clone(),"plugins-first".into(),
        false,false,callback.clone(),snapshot.clone()).unwrap();
    let first_caps = first.request(json!({"op":"capabilities"})).unwrap();
    assert_eq!(first_caps["success"],true);
    assert_eq!(first_caps["data"]["plugins"]["errors"].as_array().unwrap().len(),1);
    let nodes = first.request(json!({"op":"nodes.list"})).unwrap();
    assert_eq!(nodes["success"],true);
    assert!(nodes["data"]["nodes"].as_array().unwrap().iter().any(|node| node["typeId"] == "gain"));
    first.shutdown("Plugin snapshot reconnect test").unwrap();
    std::fs::create_dir(packages.join("invalid-added-later")).unwrap();
    std::fs::write(packages.join("invalid-added-later/manifest.json"),b"{}").unwrap();
    let second = Connection::spawn_with_plugins(&executable,fixture.0.clone(),"plugins-second".into(),
        false,false,callback,snapshot).unwrap();
    let second_caps = second.request(json!({"op":"capabilities"})).unwrap();
    assert_eq!(second_caps["success"],true);
    assert_eq!(second_caps["data"]["plugins"]["errors"].as_array().unwrap().len(),1);
    second.shutdown("Plugin snapshot reconnect test complete").unwrap();
}

#[cfg(windows)]
#[test]
fn real_sidecar_text_roundtrip_disconnect_and_session_invalidation() {
    let fixture = TempDirectory::new();
    let executable = control_executable();
    let callback: DisconnectCallback = Arc::new(|_| {});
    let first = Connection::spawn(&executable, fixture.0.clone(), "integration-1".into(),
        false, false, callback.clone()).unwrap();
    let manager = BackendManager::default();
    *manager.connection.lock().unwrap() = Some(first.clone());
    let capabilities = manager.request("integration-1", json!({"op":"capabilities", "id":"spoofed", "schema_version":999})).unwrap();
    assert_eq!(capabilities["success"], true);
    assert_ne!(capabilities["id"], "spoofed");
    let mut nested = json!(0);
    for _ in 0..70 { nested = json!([nested]); }
    for rejected_request in [
        json!({"op":"shell.exec", "command":"never run"}),
        json!({"op":"nodes.list", "padding":"x".repeat(MAX_REQUEST)}),
        json!({"op":"nodes.list", "padding":nested}),
    ] {
        let rejected = manager.request("integration-1", rejected_request).unwrap();
        assert_eq!(rejected["success"], false);
        assert_eq!(rejected["errors"][0]["code"], "desktop_request_rejected");
        assert!(first.shared.alive.load(Ordering::Acquire));
    }
    assert_eq!(manager.request("integration-1", json!({"op":"capabilities"})).unwrap()["success"], true);
    let denied = manager.request("integration-1", json!({"op":"devices.list"})).unwrap();
    assert_eq!(denied["success"], false); // 默认权限拒绝，不会枚举或开启设备。
    assert_eq!(denied["errors"][0]["code"], "device_access_denied");
    let started = manager.request("integration-1", json!({"op":"tasks.start", "mode":"offline", "graph":text_graph()})).unwrap();
    assert_eq!(started["success"], true);
    let task = started["data"]["task_id"].as_str().unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let status = manager.request("integration-1", json!({"op":"tasks.status", "task_id":task})).unwrap();
        if status["data"]["state"] == "succeeded" { break; }
        assert!(Instant::now() < deadline, "Text task failed to finish");
        thread::sleep(Duration::from_millis(2));
    }
    let result = manager.request("integration-1", json!({"op":"tasks.result", "task_id":task})).unwrap();
    assert_eq!(result["data"]["result"]["outputs"]["message"]["value"], "你好，任务接口");
    manager.request("integration-1", json!({"op":"tasks.release", "task_id":task})).unwrap();
    // 模拟RPC等待者遇到EOF清理；业务任务本身已完成，不把它声称为硬件取消测试。
    let (sender, receiver) = mpsc::channel();
    first.shared.pending.lock().unwrap().insert("waiting-at-disconnect".into(), sender);
    assert!(!manager.disconnect("integration-1").unwrap().forced);
    assert!(receiver.recv_timeout(Duration::from_secs(1)).unwrap().is_err());
    assert!(manager.request("integration-1", json!({"op":"nodes.list"})).is_err());
    assert!(first.shared.exited.lock().unwrap().is_some());

    let second = Connection::spawn(&executable, fixture.0.clone(), "integration-2".into(),
        false, false, callback).unwrap();
    *manager.connection.lock().unwrap() = Some(second.clone());
    assert!(manager.workspace("integration-1").is_err());
    assert!(manager.request("integration-1", json!({"op":"tasks.status", "task_id":task})).is_err());
    assert_eq!(manager.request("integration-2", json!({"op":"capabilities"})).unwrap()["success"], true);
    assert!(!manager.shutdown().unwrap().forced);
    assert!(manager.shutdown_complete.load(Ordering::Acquire));
    assert!(second.shared.exited.lock().unwrap().is_some());
}
