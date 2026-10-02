use super::*;

#[cfg(windows)]
#[path = "agent_live_tests.rs"]
mod live_tests;

#[cfg(windows)]
#[path = "agent_end_to_end_tests.rs"]
mod end_to_end;

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
fn workflow_prompt_describes_internal_results_without_outer_tool_envelope() {
    let request = body("mock", &VecDeque::new(), &[], "workflow");
    let system = request["messages"][0]["content"].as_str().unwrap();
    for rule in ["WITHOUT", "/steps/id/result/outputs/export_name/value", "/steps/loop/0/child",
        "/steps/choice/steps/child", "no arithmetic or string interpolation", "complete=false"] {
        assert!(system.contains(rule), "missing reference contract: {rule}");
    }
}

#[cfg(windows)]
#[test]
fn large_workflow_report_still_allows_next_model_round_and_export() {
    tauri::async_runtime::block_on(async {
        let (root,spaces,backend) = test_backend();
        let source = "x".repeat(24*1024);
        std::fs::write(spaces.ai_root.join("large.txt"), &source).unwrap();
        let program = json!({"schema_version":1,"inputs":{},"steps":[
            {"id":"a","type":"call","tool":"file_read_text","args":{"space":"ai","path":"large.txt"}},
            {"id":"b","type":"call","tool":"file_read_text","args":{"space":"ai","path":"large.txt"}},
            {"id":"c","type":"call","tool":"file_read_text","args":{"space":"ai","path":"large.txt"}}
        ],"outputs":{}});
        std::fs::write(spaces.ai_root.join("large.workflow.json"), program.to_string()).unwrap();
        let (url,received,worker) = scripted_http(vec![
            tool_response("run","workflow_run",json!({"space":"ai","path":"large.workflow.json"})),
            tool_response("export","file_export",json!({"path":"large.txt","user_path":"large-delivery/report.txt"})),
            json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"done"}}]})
        ]);
        let manager = AgentManager::default();
        let (cancel,lease) = manager.begin("large-report").unwrap();
        let store = Arc::new(RunStore::default());
        let response = run_turn(&manager,Arc::new(AiManager::default()),backend.clone(),spaces.clone(),
            "script-user".into(),"workflow".into(),"large-report".into(),"run and export".into(),
            AiConfig {base_url:url,model:"mock".into(),api_key:String::new()},None,cancel,
            Some((store.clone(),root.join("data"))),None,None).await;
        drop(lease);
        assert_eq!(response.state,"completed");
        assert_eq!((response.model_calls,response.tool_calls),(3,2));
        let _first = received.recv_timeout(Duration::from_secs(1)).unwrap();
        let second = received.recv_timeout(Duration::from_secs(1)).unwrap();
        let receipt: Value = serde_json::from_str(second["messages"].as_array().unwrap().last().unwrap()["content"].as_str().unwrap()).unwrap();
        assert_eq!(receipt["complete"],false);
        assert_eq!(receipt["ok"],true);
        assert_eq!(receipt["data"]["state"],"succeeded");
        assert!(serde_json::to_vec(&second).unwrap().len() < MAX_BODY_BYTES);
        let full = response.events.iter().find(|e|e.tool.as_deref()==Some("workflow_run")).unwrap().result.as_ref().unwrap();
        assert!(serde_json::to_vec(full).unwrap().len() > MAX_BODY_BYTES);
        let record = store.load(&spaces,&root.join("data"),receipt["data"]["run_id"].as_str().unwrap()).unwrap();
        assert_eq!(record.state,"succeeded");
        assert_eq!(std::fs::read_to_string(spaces.user_root.join("large-delivery/report.txt")).unwrap(),source);
        worker.join().unwrap();
        backend.shutdown().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    });
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
            AiConfig { base_url:url,model:"mock".into(),api_key:String::new() },None,cancel,None,None,None).await;
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
fn new_scripted_session_reads_real_history_and_writes_a_feedback_graph() {
    use crate::run_records::{RunDraft,RunFileDraft};
    use sha2::{Digest,Sha256};
    tauri::async_runtime::block_on(async {
        let (root,spaces,backend) = test_backend();
        let app_data = root.join("data");
        let store = Arc::new(RunStore::default());
        let mut wav = vec![
            b'R',b'I',b'F',b'F',44,0,0,0,b'W',b'A',b'V',b'E',b'f',b'm',b't',b' ',
            16,0,0,0,1,0,1,0,0x44,0xac,0,0,0x88,0x58,1,0,2,0,16,0,
            b'd',b'a',b't',b'a',8,0,0,0,
        ];
        for sample in [4000_i16,-4000,1000,-1000] { wav.extend_from_slice(&sample.to_le_bytes()); }
        std::fs::write(spaces.user_root.join("input.wav"),&wav).unwrap();
        let baseline_graph = json!({"schema_version":1,"nodes":[
            {"id":"input","type":"wav_input","parameters":{"path":"input.wav"}},
            {"id":"gain","type":"gain","parameters":{"gain_db":-12}},
            {"id":"meter","type":"peak_meter"},
            {"id":"output","type":"wav_output","parameters":{"path":"baseline.wav"}}
        ],"connections":[
            {"from":{"node":"input","port":"audio"},"to":{"node":"gain","port":"audio"}},
            {"from":{"node":"gain","port":"audio"},"to":{"node":"meter","port":"audio"}},
            {"from":{"node":"gain","port":"audio"},"to":{"node":"output","port":"audio"}}
        ],"exports":[{"name":"file","node":"output","port":"path"},{"name":"peak","node":"meter","port":"peak"}]});
        let baseline_configuration = json!({"mode":"offline","graph":baseline_graph,"options":{}});
        let record = store.begin(&spaces,&app_data,RunDraft {
            kind:"graph".into(),origin:"manual".into(),parent_id:None,name:"User baseline gain -12 dB".into(),
            configuration:baseline_configuration.clone(),
            files:vec![RunFileDraft { space:"user".into(),path:"input.wav".into(),role:"input".into() }],
        }).unwrap();
        let started = backend.request("script-user",json!({"op":"tasks.start","mode":"offline","graph":baseline_graph,"options":{}})).unwrap();
        assert_eq!(started["success"],true,"{started}");
        let task_id = started["data"]["task_id"].as_str().unwrap();
        let deadline = std::time::Instant::now()+Duration::from_secs(10);
        loop {
            let status = backend.request("script-user",json!({"op":"tasks.status","task_id":task_id})).unwrap();
            assert_eq!(status["success"],true,"{status}");
            match status["data"]["state"].as_str() {
                Some("succeeded") => break,
                Some("queued" | "running" | "cancelling") => {
                    assert!(std::time::Instant::now()<deadline,"Baseline Graph timed out: {status}");
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                _ => panic!("Baseline Graph did not succeed: {status}"),
            }
        }
        let result_response = backend.request("script-user",json!({"op":"tasks.result","task_id":task_id})).unwrap();
        assert_eq!(result_response["success"],true,"{result_response}");
        let baseline_result = result_response["data"].clone();
        assert_eq!(baseline_result["state"],"succeeded");
        let released = backend.request("script-user",json!({"op":"tasks.release","task_id":task_id})).unwrap();
        assert_eq!(released["success"],true,"{released}");
        let original = store.finish(&spaces,&app_data,&record.id,"succeeded",Some(baseline_result.clone()),None,
            vec![RunFileDraft { space:"user".into(),path:"baseline.wav".into(),role:"output".into() }]).unwrap();
        let original_record = serde_json::to_value(&original).unwrap();
        let original_audio = std::fs::read(spaces.user_root.join("baseline.wav")).unwrap();
        let identity = spaces.user_root.to_string_lossy().to_lowercase();
        let snapshot_path = app_data.join("run-records/v1").join(format!("{:x}",Sha256::digest(identity.as_bytes())))
            .join("records").join(format!("{}.json",record.id));
        let original_snapshot = std::fs::read(&snapshot_path).unwrap();

        // This scripted model verifies tool feedback wiring; the test fixes the edit itself.
        let mut feedback_graph = baseline_graph.clone();
        feedback_graph["nodes"][1]["parameters"]["gain_db"] = json!(-6);
        feedback_graph["nodes"][3]["parameters"]["path"] = json!("feedback.wav");
        let (url,received,worker) = scripted_http(vec![
            tool_response("history-list","runs_list",json!({"kind":"graph","state":"succeeded"})),
            tool_response("history-configuration","runs_read",json!({"id":record.id,"section":"configuration"})),
            tool_response("history-result","runs_read",json!({"id":record.id,"section":"result"})),
            tool_response("history-files","runs_check_files",json!({"id":record.id})),
            tool_response("feedback-write","file_write_text",json!({"path":"feedback.graph.json","content":feedback_graph.to_string()})),
            json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"Created feedback.graph.json with gain -6 dB and output feedback.wav."}}]}),
        ]);
        let manager = AgentManager::default();
        assert!(manager.inner.lock().unwrap().active.is_none());
        let feedback = "Start a new Graph session. My previous baseline was too quiet; inspect its saved configuration and result, then write a new Graph with gain -6 dB and a new output feedback.wav.";
        let (cancel,lease) = manager.begin("history-new-session").unwrap();
        let response = run_turn(&manager,Arc::new(AiManager::default()),backend.clone(),spaces.clone(),
            "script-user".into(),"graph".into(),"history-new-session".into(),feedback.into(),
            AiConfig { base_url:url,model:"mock".into(),api_key:String::new() },None,cancel,
            Some((store.clone(),app_data.clone())),None,None).await;
        drop(lease);
        assert_eq!(response.state,"completed","{}",response.text);
        assert_eq!((response.model_calls,response.tool_calls),(6,5));
        assert!(response.events.iter().filter(|event| event.kind=="tool").all(|event| event.success==Some(true)));
        let requests: Vec<Value> = (0..6).map(|_| received.recv_timeout(Duration::from_secs(1)).unwrap()).collect();
        worker.join().unwrap();
        let advertised: Vec<&str> = requests[0]["tools"].as_array().unwrap().iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap()).collect();
        for tool in ["runs_list","runs_read","runs_check_files"] { assert!(advertised.contains(&tool)); }
        assert!(!advertised.contains(&"graph_run"));
        assert!(!advertised.contains(&"workflow_run"));
        let first_messages = requests[0]["messages"].as_array().unwrap();
        assert_eq!(first_messages.iter().filter(|message| message["role"]=="user").count(),1);
        assert!(first_messages.iter().any(|message| message["role"]=="user" && message["content"]==feedback));
        let tool_feedback = |request: &Value, id: &str| -> Value {
            let message = request["messages"].as_array().unwrap().iter().find(|message|
                message["role"]=="tool" && message["tool_call_id"]==id).unwrap();
            serde_json::from_str(message["content"].as_str().unwrap()).unwrap()
        };
        let list_feedback = tool_feedback(&requests[1],"history-list");
        assert_eq!(list_feedback["ok"],true);
        assert_eq!(list_feedback["data"]["records"][0]["id"],record.id);
        let configuration_feedback = tool_feedback(&requests[2],"history-configuration");
        assert_eq!(configuration_feedback["ok"],true);
        assert_eq!(configuration_feedback["data"]["data"],baseline_configuration);
        let result_feedback = tool_feedback(&requests[3],"history-result");
        assert_eq!(result_feedback["ok"],true);
        let mut expected_model_result = baseline_result.clone();
        ToolContext::new(spaces.clone(),backend.clone(),"script-user".into())
            .sanitize_model_value(&mut expected_model_result);
        assert_eq!(result_feedback["data"]["data"],expected_model_result);
        let model_result_text = result_feedback["data"]["data"].to_string();
        let user_root = spaces.user_root.to_string_lossy();
        for absolute_root in [user_root.into_owned(),spaces.user_root.to_string_lossy().replace('\\',"/")] {
            let encoded_root = serde_json::to_string(&absolute_root).unwrap();
            assert!(!model_result_text.contains(&encoded_root[1..encoded_root.len()-1]),
                "Model history feedback must not contain the absolute user workspace root");
        }
        let files_feedback = tool_feedback(&requests[4],"history-files");
        assert_eq!(files_feedback["ok"],true);
        assert_eq!(files_feedback["data"]["files"].as_array().unwrap().len(),2);
        assert!(files_feedback["data"]["files"].as_array().unwrap().iter().all(|file| file["status"]=="available"));
        let write_feedback = tool_feedback(&requests[5],"feedback-write");
        assert_eq!(write_feedback["ok"],true);
        assert_eq!(write_feedback["data"]["path"],"feedback.graph.json");
        let written: Value = serde_json::from_slice(&std::fs::read(spaces.ai_root.join("feedback.graph.json")).unwrap()).unwrap();
        assert_eq!(written,feedback_graph);
        assert_eq!(store.list(&spaces,&app_data).unwrap().records.len(),1);
        assert_eq!(serde_json::to_value(store.load(&spaces,&app_data,&record.id).unwrap()).unwrap(),original_record);
        assert_eq!(std::fs::read(snapshot_path).unwrap(),original_snapshot);
        assert_eq!(std::fs::read(spaces.user_root.join("baseline.wav")).unwrap(),original_audio);
        assert_eq!(std::fs::read(spaces.user_root.join("input.wav")).unwrap(),wav);
        assert!(!spaces.ai_root.join("feedback.wav").exists());
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
            AiConfig { base_url:url,model:"mock".into(),api_key:String::new() },None,cancel,None,None,None).await;
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
            AiConfig { base_url:"http://127.0.0.1:9/v1".into(),model:"mock".into(),api_key:String::new() },None,cancel,None,None,None).await;
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
fn sixteen_model_rounds_limit_stops_without_seventeenth_request() {
    tauri::async_runtime::block_on(async {
        let (root,spaces,backend) = test_backend();
        let replies = (0..16).map(|index| tool_response(&format!("c{index}"),"workspace_list",json!({"space":"ai"}))).collect();
        let (url,received,worker) = scripted_http(replies);
        let manager = AgentManager::default();
        let (cancel,lease) = manager.begin("limit-sixteen").unwrap();
        let response = run_turn(&manager,Arc::new(AiManager::default()),backend.clone(),spaces,
            "script-user".into(),"graph".into(),"limit-sixteen".into(),"list files".into(),
            AiConfig { base_url:url,model:"mock".into(),api_key:String::new() },None,cancel,None,None,None).await;
        drop(lease);
        assert_eq!(response.state,"limited");
        assert_eq!((response.model_calls,response.tool_calls),(16,16));
        assert!(response.text.contains("16 model request limit"));
        for _ in 0..16 { received.recv_timeout(Duration::from_secs(1)).unwrap(); }
        worker.join().unwrap();
        backend.shutdown().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    });
}

#[cfg(windows)]
#[test]
fn forty_tool_calls_are_allowed_but_the_next_call_has_no_side_effect() {
    tauri::async_runtime::block_on(async {
        let (root,spaces,backend) = test_backend();
        let batches = (0..2).map(|round| {
            let mut reply = tool_response("unused","workspace_list",json!({"space":"ai"}));
            reply["choices"][0]["message"]["tool_calls"] = Value::Array((0..20).map(|index|
                json!({"id":format!("batch{round}-{index}"),"type":"function","function":{
                    "name":"workspace_list","arguments":"{\"space\":\"ai\"}"}})).collect());
            reply
        });
        let replies = batches.chain(std::iter::once(tool_response("blocked","file_write_text",
            json!({"path":"must-not-exist.txt","content":"blocked"})))).collect();
        let (url,received,worker) = scripted_http(replies);
        let manager = AgentManager::default();
        let (cancel,lease) = manager.begin("limit-forty").unwrap();
        let response = run_turn(&manager,Arc::new(AiManager::default()),backend.clone(),spaces.clone(),
            "script-user".into(),"graph".into(),"limit-forty".into(),"list files".into(),
            AiConfig { base_url:url,model:"mock".into(),api_key:String::new() },None,cancel,None,None,None).await;
        drop(lease);
        assert_eq!(response.state,"limited");
        assert_eq!((response.model_calls,response.tool_calls),(3,40));
        assert!(response.text.contains("40 tool call limit"));
        assert!(!spaces.ai_root.join("must-not-exist.txt").exists());
        for _ in 0..3 { received.recv_timeout(Duration::from_secs(1)).unwrap(); }
        worker.join().unwrap();
        backend.shutdown().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    });
}

#[cfg(windows)]
#[test]
fn persisted_conversation_continues_with_new_managers_without_reasoning_or_tool_protocol_replay() {
    tauri::async_runtime::block_on(async {
        let (root,spaces,backend) = test_backend();
        let data = root.join("data");
        let store = Arc::new(ConversationStore::default());
        let conversation = store.create(&spaces,&data,"graph",Some("Saved conversation")).unwrap();
        let mut first = tool_response("saved-write","file_write_text",json!({"path":"memory.txt","content":"remembered"}));
        first["choices"][0]["message"]["reasoning_content"] = json!("PRIVATE_REASONING_SENTINEL");
        let (url,received,worker) = scripted_http(vec![first,
            json!({"choices":[{"finish_reason":"stop","message":{"content":"Saved the remembered note."}}]})]);
        let manager = AgentManager::default();
        let (cancel,lease) = manager.begin_conversation("persist-first",Some(conversation.id.clone())).unwrap();
        let graph = json!({"schema_version":1,"nodes":[{"id":"saved-context","type":"gain","parameters":{"gain_db":-3}}],"connections":[]});
        let guard = store.begin_turn(&spaces,&data,&conversation.id,"graph","persist-first","remember this note",Some(graph.clone())).unwrap();
        let reply = run_turn(&manager,Arc::new(AiManager::default()),backend.clone(),spaces.clone(),
            "script-user".into(),"graph".into(),"persist-first".into(),"remember this note".into(),
            AiConfig { base_url:url,model:"mock".into(),api_key:"API_KEY_SENTINEL".into() },Some(graph),cancel,None,
            Some(guard),Some(conversation.id.clone())).await;
        drop(lease);
        assert_eq!(reply.state,"completed","{}",reply.text);
        let _ = received.recv_timeout(Duration::from_secs(1)).unwrap();
        let second = received.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(second["messages"].to_string().contains("PRIVATE_REASONING_SENTINEL"),"Reasoning must stay available within the current provider turn");
        worker.join().unwrap();
        drop(store); drop(manager);

        let store = Arc::new(ConversationStore::default());
        let manager = AgentManager::default();
        let loaded = store.load(&spaces,&data,&conversation.id,None,None).unwrap();
        let loaded_json = serde_json::to_string(&loaded).unwrap();
        assert!(!loaded_json.contains("PRIVATE_REASONING_SENTINEL"));
        assert!(!loaded_json.contains("API_KEY_SENTINEL"));
        assert_eq!(loaded.turns.len(),1);
        let (url,received,worker) = scripted_http(vec![
            json!({"choices":[{"finish_reason":"stop","message":{"content":"I recall the remembered note."}}]})]);
        let (cancel,lease) = manager.begin("persist-second").unwrap();
        let guard = store.begin_turn(&spaces,&data,&conversation.id,"graph","persist-second","what did we do?",None).unwrap();
        let continued = run_turn(&manager,Arc::new(AiManager::default()),backend.clone(),spaces.clone(),
            "script-user".into(),"graph".into(),"persist-second".into(),"what did we do?".into(),
            AiConfig { base_url:url,model:"mock".into(),api_key:String::new() },None,cancel,None,
            Some(guard),Some(conversation.id.clone())).await;
        drop(lease);
        assert_eq!(continued.state,"completed");
        let request = received.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
        let messages = request["messages"].as_array().unwrap();
        let text = request["messages"].to_string();
        assert!(text.contains("remember this note"));
        assert!(text.contains("Saved the remembered note."));
        assert!(text.contains("memory.txt"));
        assert!(text.contains("saved-context"));
        assert!(!text.contains("reasoning_content"));
        assert!(messages.iter().all(|m| m["role"] != "tool" && m.get("tool_calls").is_none()));
        assert!(store.begin_turn(&spaces,&data,&conversation.id,"workflow","bad-mode","no",None).is_err());
        assert!(store.begin_turn(&spaces,&data,&conversation.id,"graph","persist-first","duplicate",None).is_err());

        let other = store.create(&spaces,&data,"graph",None).unwrap();
        let (url,received,worker) = scripted_http(vec![
            json!({"choices":[{"finish_reason":"stop","message":{"content":"Fresh conversation."}}]})]);
        let (cancel,lease) = manager.begin("fresh-conversation").unwrap();
        let guard = store.begin_turn(&spaces,&data,&other.id,"graph","fresh-conversation","hello",None).unwrap();
        let fresh = run_turn(&manager,Arc::new(AiManager::default()),backend.clone(),spaces.clone(),
            "script-user".into(),"graph".into(),"fresh-conversation".into(),"hello".into(),
            AiConfig { base_url:url,model:"mock".into(),api_key:String::new() },None,cancel,None,Some(guard),Some(other.id)).await;
        drop(lease);
        assert_eq!(fresh.state,"completed");
        let request = received.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(!request["messages"].to_string().contains("remembered note"));
        worker.join().unwrap();
        let other_user = root.join("other-user"); std::fs::create_dir(&other_user).unwrap();
        let other_spaces = ToolWorkspaces::new(&other_user,&data).unwrap();
        assert!(store.load(&other_spaces,&data,&conversation.id,None,None).is_err());
        assert!(store.list(&other_spaces,&data).unwrap().records.is_empty());
        backend.shutdown().unwrap(); std::fs::remove_dir_all(root).unwrap();
    });
}

#[cfg(windows)]
#[test]
fn failed_intent_checkpoint_prevents_the_tool_and_all_later_dispatches() {
    fn locate(dir: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
        for entry in std::fs::read_dir(dir).ok()? {
            let entry = entry.ok()?;
            if entry.file_name().to_string_lossy() == name { return Some(entry.path()); }
            if entry.file_type().ok()?.is_dir() {
                if let Some(found) = locate(&entry.path(),name) { return Some(found); }
            }
        }
        None
    }
    tauri::async_runtime::block_on(async {
        let (root,spaces,backend) = test_backend(); let data = root.join("data");
        let store = Arc::new(ConversationStore::default());
        let conversation = store.create(&spaces,&data,"graph",None).unwrap();
        let guard = store.begin_turn(&spaces,&data,&conversation.id,"graph","checkpoint-fail","write",None).unwrap();
        let path = locate(&data,&format!("{}.json",conversation.id)).unwrap();
        std::fs::rename(&path,path.with_extension("saved")).unwrap();
        // Replace this test's exact record with a directory to force a read/write rejection.
        std::fs::create_dir(&path).unwrap();
        let (url,received,worker) = scripted_http(vec![tool_response("blocked-write","file_write_text",
            json!({"path":"blocked.txt","content":"must not execute"}))]);
        let manager = AgentManager::default(); let (cancel,lease) = manager.begin("checkpoint-fail").unwrap();
        let reply = run_turn(&manager,Arc::new(AiManager::default()),backend.clone(),spaces.clone(),
            "script-user".into(),"graph".into(),"checkpoint-fail".into(),"write".into(),
            AiConfig { base_url:url,model:"mock".into(),api_key:String::new() },None,cancel,None,Some(guard),Some(conversation.id)).await;
        drop(lease); received.recv_timeout(Duration::from_secs(1)).unwrap(); worker.join().unwrap();
        assert_eq!(reply.state,"failed"); assert_eq!(reply.tool_calls,0);
        assert_eq!(reply.model_calls,1); assert!(reply.text.contains("checkpoint failed before tool dispatch"));
        assert!(!spaces.ai_root.join("blocked.txt").exists());
        backend.shutdown().unwrap(); std::fs::remove_dir_all(root).unwrap();
    });
}

#[cfg(windows)]
#[test]
fn interrupted_pending_intent_load_is_read_only_and_continuation_never_automatically_repeats_tool() {
    tauri::async_runtime::block_on(async {
        let (root,spaces,backend) = test_backend(); let data = root.join("data");
        let store = Arc::new(ConversationStore::default());
        let conversation = store.create(&spaces,&data,"graph",None).unwrap();
        let mut guard = store.begin_turn(&spaces,&data,&conversation.id,"graph","crashed-turn","write once",None).unwrap();
        let arguments = json!({"path":"once.txt","content":"already executed"});
        guard.checkpoint(json!([]),Vec::new(),vec![ToolIntent::new("uncertain".into(),"file_write_text".into(),Some(arguments.clone()))]).unwrap();
        spaces.dispatch("file_write_text",&arguments).unwrap();
        drop(guard); drop(store);
        let store = Arc::new(ConversationStore::default());
        let (url,received,worker) = scripted_http(vec![
            json!({"choices":[{"finish_reason":"stop","message":{"content":"Previous call is interrupted and needs checking."}}]})]);
        let loaded = store.load(&spaces,&data,&conversation.id,None,None).unwrap();
        assert_eq!(loaded.turns[0].state,"interrupted");
        assert_eq!(loaded.turns[0].pending_tools.len(),1);
        assert!(received.try_recv().is_err(),"Loading history must not call the provider");
        assert_eq!(std::fs::read_to_string(spaces.ai_root.join("once.txt")).unwrap(),"already executed");
        let manager = AgentManager::default(); let (cancel,lease) = manager.begin("continue-after-crash").unwrap();
        let guard = store.begin_turn(&spaces,&data,&conversation.id,"graph","continue-after-crash","what happened?",None).unwrap();
        let reply = run_turn(&manager,Arc::new(AiManager::default()),backend.clone(),spaces.clone(),
            "script-user".into(),"graph".into(),"continue-after-crash".into(),"what happened?".into(),
            AiConfig { base_url:url,model:"mock".into(),api_key:String::new() },None,cancel,None,Some(guard),Some(conversation.id)).await;
        drop(lease); assert_eq!(reply.state,"completed"); assert_eq!(reply.tool_calls,0);
        let request = received.recv_timeout(Duration::from_secs(1)).unwrap(); worker.join().unwrap();
        let context = request["messages"].to_string();
        assert!(context.contains("unknown; operation may already have executed"));
        assert!(context.contains("Do not assume success or automatically retry"));
        assert!(context.contains("runs_list/runs_read/runs_check_files"));
        assert_eq!(std::fs::read_to_string(spaces.ai_root.join("once.txt")).unwrap(),"already executed");
        backend.shutdown().unwrap(); std::fs::remove_dir_all(root).unwrap();
    });
}
