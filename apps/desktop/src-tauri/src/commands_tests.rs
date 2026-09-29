use super::*;

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
    let record = store.load(&spaces,&data,&records[0].id).unwrap();
    assert_eq!(record.state,"succeeded");
    assert_eq!(record.result.as_ref().unwrap()["data"]["result"]["outputs"]["file"]["type"],"FilePath");
    assert!(record.files.iter().any(|file| file.role == "input" && file.path == "input.wav" && file.sha256.is_some()));
    assert!(record.files.iter().any(|file| file.role == "output" && file.path == "output.wav" && file.sha256.is_some()));
    manager.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
