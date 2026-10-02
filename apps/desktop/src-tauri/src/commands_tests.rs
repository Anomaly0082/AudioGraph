use super::*;

#[test]
fn terminal_recording_result_requires_matching_task_and_success_payload() {
    for result in [json!(null),json!({"outputs":{}})] {
        let response = json!({"success":true,"data":{"task_id":"task-1","state":"succeeded","result":result}});
        assert_eq!(terminal_result(&response,"task-1"),!result.is_null());
        assert!(!terminal_result(&response,"task-2"));
    }
    assert!(!terminal_result(&json!({"success":true,"data":{"task_id":"task-1","state":"succeeded"}}),"task-1"));
    for state in ["failed","cancelled"] {
        assert!(terminal_result(&json!({"success":true,"data":{"task_id":"task-1","state":state}}),"task-1"));
    }
}

#[cfg(windows)]
#[test]
fn manual_control_records_real_graph_result_and_file_hashes() {
    use crate::backend::{Connection,DisconnectCallback};
    use std::sync::atomic::{AtomicU64,Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!("audioprocess-manual-record-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
    let user = root.join("user");
    let data = root.join("data");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    let mut wav: Vec<u8> = vec![b'R',b'I',b'F',b'F',44,0,0,0,b'W',b'A',b'V',b'E',b'f',b'm',b't',b' ',
        16,0,0,0,1,0,1,0,0x44,0xac,0,0,0x88,0x58,1,0,2,0,16,0,b'd',b'a',b't',b'a',8,0,0,0];
    for sample in [4000_i16,-4000,1000,-1000] { wav.extend_from_slice(&sample.to_le_bytes()); }
    std::fs::write(user.join("input.wav"),&wav).unwrap();
    let executable_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let executable = [executable_root.join("build/Debug/control-cli.exe"),executable_root.join("build/Release/control-cli.exe")]
        .into_iter().find(|path| path.is_file()).expect("Build C++ control-cli before running integration tests");
    let spaces = ToolWorkspaces::new(&user,&data).unwrap();
    let connection = Connection::spawn(&executable,spaces.user_root.clone(),"manual-record".into(),false,false,
        Arc::new(|_| {}) as DisconnectCallback).unwrap();
    let manager = BackendManager::with_test_connection(connection);
    let store = RunStore::default();
    let tracker = ManualRunTracker::default();
    let graph = json!({"schema_version":1,"nodes":[
        {"id":"input","type":"wav_input","parameters":{"path":"input.wav"}},
        {"id":"output","type":"wav_output","parameters":{"path":"output.wav"}}],
        "connections":[{"from":{"node":"input","port":"audio"},"to":{"node":"output","port":"audio"}}],
        "exports":[{"name":"file","node":"output","port":"path"}]});
    let start = recorded_control_request(&manager,&store,&tracker,&data,"manual-record",
        json!({"op":"tasks.start","mode":"offline","graph":graph,"options":{}})).unwrap();
    assert_eq!(start["success"],true);
    let run_id = start["data"]["run_id"].as_str().unwrap();
    let task_id = start["data"]["task_id"].as_str().unwrap();
    let mut final_reply = Value::Null;
    for _ in 0..100 {
        let reply = recorded_control_request(&manager,&store,&tracker,&data,"manual-record",
            json!({"op":"tasks.status","task_id":task_id})).unwrap();
        if terminal(reply.pointer("/data/state").and_then(Value::as_str)) { final_reply = reply; break; }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(final_reply["data"]["state"],"succeeded","{final_reply}");
    assert!(final_reply.get("record_warnings").is_none());
    let records = store.list(&spaces,&data).unwrap().records;
    assert_eq!(records.len(),1);
    assert_eq!(records[0].id,run_id);
    let record = store.load(&spaces,&data,&records[0].id).unwrap();
    assert_eq!(record.state,"succeeded");
    assert_eq!(record.result.as_ref().unwrap()["data"]["result"]["outputs"]["file"]["type"],"FilePath");
    assert!(record.files.iter().any(|file| file.role == "input" && file.path == "input.wav" && file.sha256.is_some()));
    assert!(record.files.iter().any(|file| file.role == "output" && file.path == "output.wav" && file.sha256.is_some()));
    for op in ["tasks.status","tasks.result","tasks.cancel","tasks.release","tasks.result","tasks.release"] {
        let reply = recorded_control_request(&manager,&store,&tracker,&data,"manual-record",
            json!({"op":op,"task_id":task_id})).unwrap();
        assert!(reply.get("record_warnings").is_none(),"{op}: {reply}");
    }
    assert_eq!(store.load(&spaces,&data,run_id).unwrap().result,record.result);
    manager.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(windows)]
struct ManualFixture {
    root: std::path::PathBuf,
    data: std::path::PathBuf,
    spaces: ToolWorkspaces,
    manager: BackendManager,
    store: RunStore,
    tracker: ManualRunTracker,
}

#[cfg(windows)]
impl ManualFixture {
    fn new() -> Self {
        use crate::backend::{Connection,DisconnectCallback};
        use std::sync::atomic::{AtomicU64,Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!("audioprocess-manual-finalize-{}-{}",
            std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
        let user = root.join("user");
        let data = root.join("data");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        let spaces = ToolWorkspaces::new(&user,&data).unwrap();
        let executable_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let executable = [executable_root.join("build/Debug/control-cli.exe"),executable_root.join("build/Release/control-cli.exe")]
            .into_iter().find(|path| path.is_file()).expect("Build C++ control-cli before running integration tests");
        let connection = Connection::spawn(&executable,spaces.user_root.clone(),"manual-finalize".into(),false,false,
            Arc::new(|_| {}) as DisconnectCallback).unwrap();
        Self { root,data,spaces,manager:BackendManager::with_test_connection(connection),
            store:RunStore::default(),tracker:ManualRunTracker::default() }
    }

    fn request(&self, op: &str, task_id: &str) -> Value {
        recorded_control_request(&self.manager,&self.store,&self.tracker,&self.data,"manual-finalize",
            json!({"op":op,"task_id":task_id})).unwrap()
    }

    // Seed a tracked run without observing the start's state. This deterministically
    // exercises release before persistence even when the real C++ task finishes fast.
    fn pending(&self, graph: Value) -> (String,String) {
        let request = json!({"op":"tasks.start","mode":"offline","graph":graph,"options":{}});
        let record = self.store.begin(&self.spaces,&self.data,RunDraft { kind:"graph".into(),origin:"manual".into(),
            parent_id:None,name:"Finalization test".into(),configuration:request.clone(),files:vec![] }).unwrap();
        let reply = self.manager.request("manual-finalize",request).unwrap();
        assert_eq!(reply["success"],true,"{reply}");
        let task_id = reply["data"]["task_id"].as_str().unwrap().to_owned();
        self.tracker.tasks.lock().unwrap().insert(("manual-finalize".into(),task_id.clone()),
            PendingRun { id:record.id.clone(),outputs:vec![] });
        (task_id,record.id)
    }

    fn wait(&self, task_id: &str) -> Value {
        for _ in 0..100 {
            let reply = self.manager.request("manual-finalize",json!({"op":"tasks.status","task_id":task_id})).unwrap();
            if terminal(reply.pointer("/data/state").and_then(Value::as_str)) { return reply; }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("Task did not finish");
    }
}

#[cfg(windows)]
impl Drop for ManualFixture {
    fn drop(&mut self) {
        self.manager.shutdown().unwrap();
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[cfg(windows)]
fn text_graph() -> Value {
    json!({"schema_version":1,"nodes":[{"id":"text","type":"text_input","parameters":{"text":"finalize"}}],
        "connections":[],"exports":[{"name":"message","node":"text","port":"text"}]})
}

#[cfg(windows)]
#[test]
fn release_before_result_persists_real_terminal_outcome() {
    let fixture = ManualFixture::new();
    let (task_id,run_id) = fixture.pending(text_graph());
    assert_eq!(fixture.wait(&task_id)["data"]["state"],"succeeded");
    assert_eq!(fixture.store.load(&fixture.spaces,&fixture.data,&run_id).unwrap().state,"running");
    let reply = fixture.request("tasks.release",&task_id);
    assert_eq!(reply["success"],true,"{reply}");
    assert_eq!(reply["data"]["released"],true);
    assert!(reply.get("record_warnings").is_none(),"{reply}");
    let record = fixture.store.load(&fixture.spaces,&fixture.data,&run_id).unwrap();
    assert_eq!(record.state,"succeeded");
    assert_eq!(record.result.unwrap()["data"]["result"]["outputs"]["message"]["value"],"finalize");
    assert!(fixture.tracker.tasks.lock().unwrap().is_empty());
    assert_eq!(fixture.request("tasks.status",&task_id)["success"],false);
}

#[cfg(windows)]
#[test]
fn failed_task_result_without_payload_can_be_persisted_and_released() {
    let fixture = ManualFixture::new();
    let graph = json!({"schema_version":1,"nodes":[{"id":"input","type":"wav_input","parameters":{"path":"missing.wav"}}],
        "connections":[],"exports":[{"name":"audio","node":"input","port":"audio"}]});
    let (task_id,run_id) = fixture.pending(graph);
    assert_eq!(fixture.wait(&task_id)["data"]["state"],"failed");
    let result = fixture.request("tasks.result",&task_id);
    assert_eq!(result["success"],true,"{result}");
    assert!(result.pointer("/data/result").is_none());
    assert!(result.get("record_warnings").is_none(),"{result}");
    let release = fixture.request("tasks.release",&task_id);
    assert_eq!(release["success"],true,"{release}");
    assert!(release.get("record_warnings").is_none(),"{release}");
    let record = fixture.store.load(&fixture.spaces,&fixture.data,&run_id).unwrap();
    assert_eq!(record.state,"failed");
    assert!(record.error.is_some());
    assert_eq!(record.result.unwrap()["data"]["errors"],result["data"]["errors"]);
}

#[cfg(windows)]
#[test]
fn release_keeps_backend_result_and_tracker_when_record_save_fails() {
    let fixture = ManualFixture::new();
    let (task_id,run_id) = fixture.pending(text_graph());
    fixture.wait(&task_id);
    // Keep the original running record recoverable, but make its target a directory.
    let record_path = std::fs::read_dir(fixture.data.join("run-records/v1")).unwrap().next().unwrap().unwrap().path()
        .join("records").join(format!("{run_id}.json"));
    let backup = record_path.with_extension("saved");
    std::fs::rename(&record_path,&backup).unwrap();
    std::fs::create_dir(&record_path).unwrap();
    let release = fixture.request("tasks.release",&task_id);
    assert_eq!(release["success"],false,"{release}");
    assert_eq!(release["errors"][0]["code"],"run_record_finalize_failed");
    assert!(release["record_warnings"].as_array().is_some_and(|warnings| !warnings.is_empty()));
    assert!(fixture.tracker.tasks.lock().unwrap().contains_key(&("manual-finalize".into(),task_id.clone())));
    let retained = fixture.manager.request("manual-finalize",json!({"op":"tasks.result","task_id":task_id})).unwrap();
    assert_eq!(retained["success"],true,"{retained}");
    std::fs::remove_dir(&record_path).unwrap();
    std::fs::rename(&backup,&record_path).unwrap();
    let retry = fixture.request("tasks.release",&task_id);
    assert_eq!(retry["success"],true,"{retry}");
    assert!(retry.get("record_warnings").is_none(),"{retry}");
    assert_eq!(fixture.store.load(&fixture.spaces,&fixture.data,&run_id).unwrap().state,"succeeded");
}

#[cfg(windows)]
#[test]
fn missing_terminal_result_blocks_release_and_keeps_history_pending() {
    let fixture = ManualFixture::new();
    let (task_id,run_id) = fixture.pending(text_graph());
    fixture.wait(&task_id);
    // Simulate the result disappearing outside the recorded control path.
    let removed = fixture.manager.request("manual-finalize",json!({"op":"tasks.release","task_id":task_id})).unwrap();
    assert_eq!(removed["success"],true);
    let release = fixture.request("tasks.release",&task_id);
    assert_eq!(release["success"],false,"{release}");
    assert_eq!(release["errors"][0]["code"],"run_record_finalize_failed");
    assert!(release["record_warnings"].as_array().is_some_and(|warnings| !warnings.is_empty()));
    assert!(fixture.tracker.tasks.lock().unwrap().contains_key(&("manual-finalize".into(),task_id)));
    let record = fixture.store.load(&fixture.spaces,&fixture.data,&run_id).unwrap();
    assert_eq!(record.state,"running");
    assert!(record.result.is_none());
}

#[cfg(windows)]
#[test]
fn concurrent_terminal_queries_and_release_do_not_report_duplicate_record_errors() {
    let fixture = ManualFixture::new();
    let (task_id,run_id) = fixture.pending(text_graph());
    fixture.wait(&task_id);
    let barrier = std::sync::Barrier::new(12);
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for index in 0..12 {
            let fixture = &fixture;
            let barrier = &barrier;
            let task_id = &task_id;
            handles.push(scope.spawn(move || {
                barrier.wait();
                let op = ["tasks.status","tasks.result","tasks.cancel","tasks.release"][index % 4];
                let reply = fixture.request(op,task_id);
                assert!(reply.get("record_warnings").is_none(),"{op}: {reply}");
                reply
            }));
        }
        let replies: Vec<_> = handles.into_iter().map(|handle| handle.join().unwrap()).collect();
        assert!(replies.iter().any(|reply| reply.pointer("/data/released") == Some(&json!(true))));
    });
    let record = fixture.store.load(&fixture.spaces,&fixture.data,&run_id).unwrap();
    assert_eq!(record.state,"succeeded");
    assert!(record.result.is_some());
    assert!(fixture.tracker.tasks.lock().unwrap().is_empty());
}
