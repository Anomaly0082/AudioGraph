use super::*;

#[test]
fn active_turn_cancel_and_reset_are_serialized() {
    let manager = AgentManager::default();
    let (cancel,lease) = manager.begin("request").unwrap();
    assert!(manager.begin("other").is_err());
    assert!(manager.reset("session","graph").is_err());
    assert!(manager.cancel("request"));
    assert!(cancel.load(Ordering::Acquire));
    drop(lease);
    manager.wait_idle(Duration::from_millis(1)).unwrap();
    manager.reset("session","graph").unwrap();
    manager.shutdown();
    assert!(manager.begin("later").is_err());
    manager.reopen();
    assert!(manager.begin("later").is_ok());
}

#[test]
fn cancel_before_begin_is_remembered_once_and_bounded() {
    let manager = AgentManager::default();
    assert!(manager.cancel("queued"));
    assert!(manager.begin("queued").is_err());
    assert!(manager.begin("queued").is_ok());
    for index in 0..70 { assert!(manager.cancel(&format!("id-{index}"))); }
    assert_eq!(manager.inner.lock().unwrap().pre_cancelled.len(),64);
    assert!(manager.begin("id-0").is_ok());
    assert!(manager.begin("id-69").is_err());
}

#[test]
fn response_shape_and_request_only_reasoning_are_handled() {
    let response = json!({"choices":[{"finish_reason":"tool_calls","message":{
        "role":"assistant","content":null,"reasoning_content":"private",
        "tool_calls":[{"id":"c1","type":"function","function":{"name":"workspace_list","arguments":"{\"space\":\"ai\"}"}}]
    }}]});
    let message = response_message(&response).unwrap();
    assert_eq!(content(message).unwrap(),"");
    let followup = assistant_followup(message,message["tool_calls"].as_array().unwrap());
    assert_eq!(followup["reasoning_content"],"private");
    assert!(serde_json::to_string(&AgentEvent::message("assistant","visible".into())).unwrap().find("private").is_none());
    assert!(response_message(&json!({"choices":[{"finish_reason":"length","message":{}}]})).is_err());
}

#[test]
fn request_body_keeps_tool_pairs_and_reports_size_to_caller() {
    let mut current = vec![json!({"role":"user","content":"hello"})];
    current.push(json!({"role":"assistant","content":null,"tool_calls":[{"id":"c","type":"function","function":{"name":"file_read_text","arguments":"{}"}}]}));
    current.push(json!({"role":"tool","tool_call_id":"c","content":"{\"ok\":false}"}));
    let message = body("mock",&VecDeque::new(),&current,"graph");
    let messages = message["messages"].as_array().unwrap();
    assert_eq!(messages.len(),4);
    assert_eq!(messages[3]["tool_call_id"],"c");
    assert!(serde_json::to_vec(&message).unwrap().len() < MAX_BODY_BYTES);
}

#[test]
fn unsuccessful_workflow_report_is_a_failed_tool_envelope_with_partial_trace() {
    let partial = json!({"schema_version":1,"state":"failed","outputs":{},
        "step_results":{"before":{"path":"before.txt"}},"trace":[{"step_path":"/steps/0","state":"succeeded"}],
        "error":{"code":"tool_failed","message":"File does not exist","step_path":"/steps/1"}});
    let (envelope,success) = tool_envelope("workflow_run",Ok(partial.clone()));
    assert!(!success);
    assert_eq!(envelope["ok"],false);
    assert_eq!(envelope["data"],partial);
    assert_eq!(envelope["error"]["code"],"tool_failed");
    let (envelope,success) = tool_envelope("workflow_run",Ok(json!({"state":"succeeded","outputs":{}})));
    assert!(success);
    assert_eq!(envelope["ok"],true);
}

#[cfg(windows)]
fn test_backend() -> (std::path::PathBuf,ToolWorkspaces,Arc<BackendManager>) {
    use crate::backend::{Connection,DisconnectCallback};
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!("audioprocess-agent-loop-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
    let user = root.join("user");
    let data = root.join("data");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    let spaces = ToolWorkspaces::new(&user,&data).unwrap();
    let executable_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let executable = [executable_root.join("build/Debug/control-cli.exe"),executable_root.join("build/Release/control-cli.exe")]
        .into_iter().find(|p| p.is_file()).expect("Build C++ control-cli before desktop integration tests");
    let callback: DisconnectCallback = Arc::new(|_| {});
    let connection = Connection::spawn(&executable,spaces.user_root.clone(),"script-user".into(),false,false,callback).unwrap();
    (root,spaces,Arc::new(BackendManager::with_test_connection(connection)))
}

#[cfg(windows)]
fn scripted_http(replies: Vec<Value>) -> (String,std::sync::mpsc::Receiver<Value>,std::thread::JoinHandle<()>) {
    use std::io::{Read,Write};
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1",listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let (sender,receiver) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        for reply in replies {
            let deadline = std::time::Instant::now()+Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream,_)) => break stream,
                    Err(error) if error.kind()==std::io::ErrorKind::WouldBlock && std::time::Instant::now()<deadline => std::thread::sleep(Duration::from_millis(2)),
                    Err(error) => panic!("Mock model accept failed: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0_u8;4096];
            loop {
                let count = stream.read(&mut buffer).unwrap();
                if count==0 { break; }
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(end) = bytes.windows(4).position(|slice| slice==b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    let length = headers.lines().find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())).unwrap_or(0);
                    if bytes.len()>=end+4+length {
                        sender.send(serde_json::from_slice::<Value>(&bytes[end+4..end+4+length]).unwrap()).unwrap();
                        break;
                    }
                }
            }
            let response = reply.to_string();
            let header = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",response.len());
            stream.write_all(header.as_bytes()).unwrap();
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    (url,receiver,worker)
}

#[cfg(windows)]
fn tool_response(id: &str, name: &str, args: Value) -> Value {
    json!({"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
        "tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}}]}}]})
}

#[cfg(windows)]
#[test]
fn scripted_model_receives_file_tool_results_and_finishes() {
    tauri::async_runtime::block_on(async {
        let (root,spaces,backend) = test_backend();
        let (url,received,worker) = scripted_http(vec![
            tool_response("write","file_write_text",json!({"path":"note.txt","content":"hello"})),
            tool_response("read","file_read_text",json!({"space":"ai","path":"note.txt"})),
            json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"done"}}]}),
        ]);
        let manager = AgentManager::default();
        let (cancel,lease) = manager.begin("script-1").unwrap();
        let response = run_turn(&manager,Arc::new(AiManager::default()),backend.clone(),spaces.clone(),
            "script-user".into(),"graph".into(),"script-1".into(),"make a note".into(),
            AiConfig { base_url:url,model:"mock".into(),api_key:String::new() },None,cancel,None).await;
        drop(lease);
        assert_eq!(response.state,"completed");
        assert_eq!((response.model_calls,response.tool_calls),(3,2));
        assert_eq!(std::fs::read_to_string(spaces.ai_root.join("note.txt")).unwrap(),"hello");
        let _first = received.recv_timeout(Duration::from_secs(1)).unwrap();
        let second = received.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(second["messages"].to_string().contains("\\\"ok\\\":true"));
        let third = received.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(third["messages"].to_string().contains("hello"));
        worker.join().unwrap();
        backend.shutdown().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    });
}

#[cfg(windows)]
#[test]
fn scripted_tool_failure_is_feedback_not_success() {
    tauri::async_runtime::block_on(async {
        let (root,spaces,backend) = test_backend();
        let (url,received,worker) = scripted_http(vec![
            tool_response("missing","file_read_text",json!({"space":"ai","path":"absent.txt"})),
            json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"The file is missing."}}]}),
        ]);
        let manager = AgentManager::default();
        let (cancel,lease) = manager.begin("script-2").unwrap();
        let response = run_turn(&manager,Arc::new(AiManager::default()),backend.clone(),spaces,
            "script-user".into(),"graph".into(),"script-2".into(),"read absent".into(),
            AiConfig { base_url:url,model:"mock".into(),api_key:String::new() },None,cancel,None).await;
        drop(lease);
        assert_eq!(response.state,"completed");
        assert_eq!(response.events.iter().find(|e| e.kind=="tool").unwrap().success,Some(false));
        let _first = received.recv_timeout(Duration::from_secs(1)).unwrap();
        let second = received.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(second["messages"].to_string().contains("\\\"ok\\\":false"));
        worker.join().unwrap();
        backend.shutdown().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    });
}

#[cfg(windows)]
#[test]
fn cancelled_turn_makes_no_model_or_file_call() {
    tauri::async_runtime::block_on(async {
        let (root,spaces,backend) = test_backend();
        let manager = AgentManager::default();
        let (cancel,lease) = manager.begin("cancel-now").unwrap();
        assert!(manager.cancel("cancel-now"));
        let response = run_turn(&manager,Arc::new(AiManager::default()),backend.clone(),spaces.clone(),
            "script-user".into(),"graph".into(),"cancel-now".into(),"write a note".into(),
            AiConfig { base_url:"http://127.0.0.1:9/v1".into(),model:"mock".into(),api_key:String::new() },None,cancel,None).await;
        drop(lease);
        assert_eq!(response.state,"cancelled");
        assert_eq!((response.model_calls,response.tool_calls),(0,0));
        assert!(!spaces.ai_root.join("note.txt").exists());
        backend.shutdown().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    });
}

#[cfg(windows)]
#[test]
fn eight_model_rounds_limit_stops_without_ninth_request() {
    tauri::async_runtime::block_on(async {
        let (root,spaces,backend) = test_backend();
        let replies = (0..8).map(|index| tool_response(&format!("c{index}"),"workspace_list",json!({"space":"ai"}))).collect();
        let (url,received,worker) = scripted_http(replies);
        let manager = AgentManager::default();
        let (cancel,lease) = manager.begin("limit-eight").unwrap();
        let response = run_turn(&manager,Arc::new(AiManager::default()),backend.clone(),spaces,
            "script-user".into(),"graph".into(),"limit-eight".into(),"list files".into(),
            AiConfig { base_url:url,model:"mock".into(),api_key:String::new() },None,cancel,None).await;
        drop(lease);
        assert_eq!(response.state,"limited");
        assert_eq!((response.model_calls,response.tool_calls),(8,8));
        for _ in 0..8 { received.recv_timeout(Duration::from_secs(1)).unwrap(); }
        worker.join().unwrap();
        backend.shutdown().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    });
}
