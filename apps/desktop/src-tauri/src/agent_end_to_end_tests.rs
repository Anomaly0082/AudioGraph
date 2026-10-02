use super::*;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

/// The production locator intentionally looks beside current_exe. Install the already-built
/// fixture there without replacing a different binary; both local agent tests and the separate
/// opt-in live smoke can use this helper. The target is a Cargo build artifact, never user data.
pub(super) fn ensure_test_sidecar() -> PathBuf {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let source = [repo.join("build/Debug/control-cli.exe"), repo.join("build/Release/control-cli.exe")]
        .into_iter().find(|path| path.is_file()).expect("Build C++ control-cli before desktop integration tests");
    let target = std::env::current_exe().unwrap().parent().unwrap().join("control-cli.exe");
    match OpenOptions::new().write(true).create_new(true).open(&target) {
        Ok(mut destination) => {
            let mut input = fs::File::open(&source).unwrap();
            std::io::copy(&mut input,&mut destination).unwrap();
            destination.sync_all().unwrap();
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => panic!("Cannot prepare existing C++ sidecar fixture: {error}"),
    }
    let source_hash = Sha256::digest(fs::read(source).unwrap());
    let target_hash = Sha256::digest(fs::read(&target).unwrap());
    assert_eq!(source_hash,target_hash,"Existing test sidecar differs from the built C++ fixture; rebuild or remove that exact stale test artifact");
    target
}

fn tiny_wav() -> Vec<u8> {
    let mut bytes = vec![
        b'R',b'I',b'F',b'F',44,0,0,0,b'W',b'A',b'V',b'E',b'f',b'm',b't',b' ',
        16,0,0,0,1,0,1,0,0x44,0xac,0,0,0x88,0x58,1,0,2,0,16,0,
        b'd',b'a',b't',b'a',8,0,0,0,
    ];
    for sample in [10_000_i16,-10_000,2_500,-2_500] { bytes.extend_from_slice(&sample.to_le_bytes()); }
    bytes
}

fn workflow(gain_db: f64, output: &str) -> Value {
    json!({
        "schema_version":1,
        "inputs":{"gain_db":gain_db,"output":output},
        "steps":[{"id":"render","type":"call","tool":"graph_run","args":{
            "mode":"offline","graph":{
                "schema_version":1,
                "nodes":[
                    {"id":"input","type":"wav_input","parameters":{"path":"trial/input.wav"}},
                    {"id":"gain","type":"gain","parameters":{"gain_db":{"$ref":"/inputs/gain_db"}}},
                    {"id":"meter","type":"peak_meter"},
                    {"id":"output","type":"wav_output","parameters":{"path":{"$ref":"/inputs/output"}}}
                ],
                "connections":[
                    {"from":{"node":"input","port":"audio"},"to":{"node":"gain","port":"audio"}},
                    {"from":{"node":"gain","port":"audio"},"to":{"node":"meter","port":"audio"}},
                    {"from":{"node":"gain","port":"audio"},"to":{"node":"output","port":"audio"}}
                ],
                "exports":[
                    {"name":"peak","node":"meter","port":"peak"},
                    {"name":"file","node":"output","port":"path"}
                ]
            }
        }}],
        "outputs":{"render":{"$ref":"/steps/render"}}
    })
}

fn model_calls(calls: Vec<(&str,&str,Value)>) -> Value {
    let tools: Vec<Value> = calls.into_iter().map(|(id,name,args)|
        json!({"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}})).collect();
    model_call_values(tools)
}

fn model_call_values(tools: Vec<Value>) -> Value {
    json!({"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":tools}}]})
}

fn tool_feedback(request: &Value, id: &str) -> Value {
    let content = request["messages"].as_array().unwrap().iter().find(|message|
        message["role"] == "tool" && message["tool_call_id"] == id)
        .unwrap_or_else(|| panic!("Next model request omitted feedback for {id}"))["content"].as_str().unwrap();
    serde_json::from_str(content).unwrap()
}

fn successful(request: &Value, id: &str) -> Value {
    let envelope = tool_feedback(request,id);
    assert_eq!(envelope["ok"],true,"{id} failed: {envelope}");
    envelope["data"].clone()
}

fn workflow_peak(report: &Value) -> f64 {
    assert_eq!(report["state"],"succeeded","{report}");
    assert_eq!(report["graph_runs"],1,"{report}");
    report.pointer("/outputs/render/result/outputs/peak/value").and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("Workflow did not return a numeric Graph peak: {report}"))
}

fn read_request(stream: &mut std::net::TcpStream) -> Value {
    stream.set_nonblocking(false).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut bytes = Vec::new();
    let mut buffer = [0_u8;4096];
    loop {
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0,"Model HTTP request ended before its JSON body");
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(end) = bytes.windows(4).position(|slice| slice == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..end]);
            let length = headers.lines().find_map(|line|
                line.to_ascii_lowercase().strip_prefix("content-length:")
                    .and_then(|value| value.trim().parse::<usize>().ok())).unwrap();
            if bytes.len() >= end+4+length {
                return serde_json::from_slice(&bytes[end+4..end+4+length]).unwrap();
            }
        }
    }
}

struct ServerTrace { requests: Vec<Value>, baseline_peak: f64, adjusted_peak: f64, adjusted_gain_db: f64 }

fn localhost_model() -> (String, thread::JoinHandle<ServerTrace>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1",listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let worker = thread::spawn(move || {
        let mut requests = Vec::new();
        let mut baseline_peak = None;
        let mut adjusted_peak = None;
        let mut adjusted_gain_db = None;
        for round in 0..9 {
            let deadline = Instant::now()+Duration::from_secs(20);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream,_)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                        thread::sleep(Duration::from_millis(5)),
                    Err(error) => panic!("Local model did not receive round {round}: {error}"),
                }
            };
            let request = read_request(&mut stream);
            assert_eq!(request["tool_choice"],"auto");
            let response = match round {
                0 => {
                    let advertised: Vec<_> = request["tools"].as_array().unwrap().iter()
                        .filter_map(|tool| tool.pointer("/function/name").and_then(Value::as_str)).collect();
                    for name in ["workspace_list","nodes_list","file_copy_to_ai","workflow_validate","workflow_run","runs_list"] {
                        assert!(advertised.contains(&name),"Tool {name} not advertised");
                    }
                    model_calls(vec![("user-list","workspace_list",json!({"space":"user"})),
                        ("ai-list","workspace_list",json!({"space":"ai"})),
                        ("nodes","nodes_list",json!({}))])
                }
                1 => {
                    let user = successful(&request,"user-list");
                    let ai = successful(&request,"ai-list");
                    let nodes = successful(&request,"nodes");
                    assert!(user["entries"].as_array().unwrap().iter().any(|entry| entry["name"] == "input.wav"));
                    assert!(!ai["entries"].as_array().unwrap().iter().any(|entry| entry["name"] == "input.wav"));
                    let catalog = nodes["nodes"].as_array().unwrap();
                    for kind in ["wav_input","gain","peak_meter","wav_output"] {
                        assert!(catalog.iter().any(|node| node["typeId"] == kind));
                    }
                    model_calls(vec![("make-trial","directory_create",json!({"path":"trial"})),
                        ("import","file_copy_to_ai",json!({"source_space":"user","source_path":"input.wav","path":"trial/input.wav"}))])
                }
                2 => {
                    assert_eq!(successful(&request,"make-trial")["path"],"trial");
                    assert_eq!(successful(&request,"import")["path"],"trial/input.wav");
                    model_calls(vec![("save-baseline","file_write_text",json!({
                        "path":"trial/baseline.workflow.json","content":workflow(0.0,"trial/baseline.wav").to_string()
                    })),("validate-baseline","workflow_validate",json!({"space":"ai","path":"trial/baseline.workflow.json"}))])
                }
                3 => {
                    assert_eq!(successful(&request,"save-baseline")["path"],"trial/baseline.workflow.json");
                    assert_eq!(successful(&request,"validate-baseline")["valid"],true);
                    model_calls(vec![("run-baseline","workflow_run",json!({"space":"ai","path":"trial/baseline.workflow.json"}))])
                }
                4 => {
                    let report = successful(&request,"run-baseline");
                    let peak = workflow_peak(&report);
                    assert!(peak > 0.25 && peak < 0.35,"Unexpected measured baseline peak: {peak}");
                    baseline_peak = Some(peak);
                    // This decision depends on the actual C++ measurement returned through
                    // tool feedback. The response is not a prewritten successful continuation.
                    let gain = 20.0 * (0.18_f64/peak).log10();
                    assert!(gain < 0.0 && gain > -20.0);
                    adjusted_gain_db = Some(gain);
                    model_calls(vec![("save-adjusted","file_write_text",json!({
                        "path":"trial/adjusted.workflow.json","content":workflow(gain,"trial/adjusted.wav").to_string()
                    })),("validate-adjusted","workflow_validate",json!({"space":"ai","path":"trial/adjusted.workflow.json"}))])
                }
                5 => {
                    assert_eq!(successful(&request,"save-adjusted")["path"],"trial/adjusted.workflow.json");
                    assert_eq!(successful(&request,"validate-adjusted")["valid"],true);
                    model_calls(vec![("run-adjusted","workflow_run",json!({"space":"ai","path":"trial/adjusted.workflow.json"}))])
                }
                6 => {
                    let report = successful(&request,"run-adjusted");
                    let peak = workflow_peak(&report);
                    assert!(peak < baseline_peak.unwrap()*0.8,"Gain adjustment did not lower measured peak: {peak}");
                    adjusted_peak = Some(peak);
                    model_calls(vec![
                        ("export-baseline","file_export",json!({"path":"trial/baseline.wav","user_path":"acceptance-output-001/baseline.wav"})),
                        ("export-adjusted","file_export",json!({"path":"trial/adjusted.wav","user_path":"acceptance-output-001/adjusted.wav"})),
                        ("duplicate-export","file_export",json!({"path":"trial/adjusted.wav","user_path":"acceptance-output-001/baseline.wav"})),
                        ("history","runs_list",json!({"limit":10}))])
                }
                7 => {
                    assert_eq!(successful(&request,"export-baseline")["path"],"acceptance-output-001/baseline.wav");
                    assert_eq!(successful(&request,"export-adjusted")["path"],"acceptance-output-001/adjusted.wav");
                    let rejected = tool_feedback(&request,"duplicate-export");
                    assert_eq!(rejected["ok"],false,"Duplicate export unexpectedly succeeded: {rejected}");
                    let history = successful(&request,"history");
                    assert_eq!(history["history_incomplete"],false);
                    let records = history["records"].as_array().unwrap();
                    assert_eq!(records.len(),4,"Expected two workflow parents and two Graph children: {history}");
                    let parents: Vec<_> = records.iter().filter(|record| record["kind"] == "workflow").collect();
                    let children: Vec<_> = records.iter().filter(|record| record["kind"] == "graph").collect();
                    assert_eq!((parents.len(),children.len()),(2,2));
                    assert!(parents.iter().all(|parent| parent["state"] == "succeeded" && parent["parent_id"].is_null()));
                    assert!(children.iter().all(|child| child["state"] == "succeeded" &&
                        parents.iter().any(|parent| child["parent_id"] == parent["id"])));
                    model_call_values(records.iter().map(|record| {
                        let id = record["id"].as_str().unwrap();
                        let call_id = if record["kind"] == "workflow" { "read-workflow" } else { "read-graph" };
                        json!({"id":format!("{call_id}-{id}"),"type":"function","function":{
                            "name":"runs_read","arguments":json!({"id":id,"section":"summary"}).to_string()}})
                    }).collect())
                }
                8 => {
                    let previous = &requests[7];
                    let records = successful(previous,"history")["records"].as_array().unwrap().clone();
                    for record in records {
                        let id = record["id"].as_str().unwrap();
                        let prefix = if record["kind"] == "workflow" { "read-workflow" } else { "read-graph" };
                        let detail = successful(&request,&format!("{prefix}-{id}"));
                        assert_eq!(detail["data"]["id"],id);
                    }
                    json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant",
                        "content":format!("Compared two actual Workflow runs: peak {:.6} -> {:.6}. Adjusted gain {:.3} dB; exports are in acceptance-output-001. Duplicate export was rejected.",
                            baseline_peak.unwrap(),adjusted_peak.unwrap(),adjusted_gain_db.unwrap())}}]})
                }
                _ => unreachable!(),
            };
            requests.push(request);
            let text = response.to_string();
            let header = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",text.len());
            stream.write_all(header.as_bytes()).unwrap();
            stream.write_all(text.as_bytes()).unwrap();
        }
        ServerTrace { requests, baseline_peak:baseline_peak.unwrap(), adjusted_peak:adjusted_peak.unwrap(),
            adjusted_gain_db:adjusted_gain_db.unwrap() }
    });
    (url,worker)
}

fn sample_peak(bytes: &[u8]) -> i16 {
    bytes[44..].chunks_exact(2).map(|sample| i16::from_le_bytes([sample[0],sample[1]]).abs()).max().unwrap()
}

#[test]
fn localhost_protocol_exercises_adaptive_workflow_and_linked_runs() {
    tauri::async_runtime::block_on(async {
        let _sidecar = ensure_test_sidecar();
        let (root,spaces,backend) = super::test_backend();
        let original = tiny_wav();
        fs::write(spaces.user_root.join("input.wav"),&original).unwrap();
        let store = Arc::new(RunStore::default());
        let (url,model) = localhost_model();
        let manager = AgentManager::default();
        let (cancel,lease) = manager.begin("end-to-end-local").unwrap();
        let answer = run_turn(&manager,Arc::new(AiManager::default()),backend.clone(),spaces.clone(),
            "script-user".into(),"workflow".into(),"end-to-end-local".into(),
            "Use the small local input WAV, measure a baseline, adjust gain from the measured peak, then export both outputs.".into(),
            AiConfig { base_url:url,model:"local-script".into(),api_key:String::new() },None,cancel,
            Some((store.clone(),root.join("data"))),None,None).await;
        drop(lease);
        let trace = model.join().unwrap();
        assert_eq!(answer.state,"completed","{}",answer.text);
        assert_eq!((answer.model_calls,answer.tool_calls),(9,19));
        assert_eq!(trace.requests.len(),9);
        assert!(trace.adjusted_gain_db < 0.0);
        assert!(trace.adjusted_peak < trace.baseline_peak*0.8);
        let adjusted_program: Value = serde_json::from_slice(&fs::read(spaces.ai_root.join("trial/adjusted.workflow.json")).unwrap()).unwrap();
        assert_eq!(adjusted_program["inputs"]["gain_db"].as_f64().unwrap(),trace.adjusted_gain_db);
        assert!(answer.text.contains("Duplicate export was rejected"));
        let tools: Vec<_> = answer.events.iter().filter(|event| event.kind == "tool").collect();
        assert_eq!(tools.len(),19);
        assert_eq!(tools.iter().filter(|event| event.success == Some(false)).count(),1);
        assert_eq!(tools.iter().find(|event| event.tool.as_deref() == Some("file_export") && event.success == Some(false))
            .unwrap().result.as_ref().unwrap()["ok"],false);
        assert_eq!(fs::read(spaces.user_root.join("input.wav")).unwrap(),original);
        assert_eq!(fs::read(spaces.ai_root.join("trial/input.wav")).unwrap(),original);
        let baseline = fs::read(spaces.ai_root.join("trial/baseline.wav")).unwrap();
        let adjusted = fs::read(spaces.ai_root.join("trial/adjusted.wav")).unwrap();
        assert!(baseline.starts_with(b"RIFF") && adjusted.starts_with(b"RIFF"));
        assert!(sample_peak(&adjusted) < sample_peak(&baseline));
        let delivered = spaces.user_root.join("acceptance-output-001");
        assert_eq!(fs::read(delivered.join("baseline.wav")).unwrap(),baseline,
            "Rejected overwrite must preserve the first exported file");
        assert_eq!(fs::read(delivered.join("adjusted.wav")).unwrap(),adjusted);
        assert_eq!(fs::read_dir(&delivered).unwrap().count(),2);
        let records = store.list(&spaces,&root.join("data")).unwrap().records;
        assert_eq!(records.len(),4);
        let parents: Vec<_> = records.iter().filter(|record| record.kind == "workflow").collect();
        let children: Vec<_> = records.iter().filter(|record| record.kind == "graph").collect();
        assert_eq!((parents.len(),children.len()),(2,2));
        assert!(children.iter().all(|child| parents.iter().any(|parent| child.parent_id.as_deref() == Some(parent.id.as_str()))));
        assert!(answer.run_ids.iter().all(|id| records.iter().any(|record| &record.id == id)));
        assert!(answer.run_ids.len() >= 4);
        backend.shutdown().unwrap();
        fs::remove_dir_all(root).unwrap();
    });
}
