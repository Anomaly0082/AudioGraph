use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}, mpsc};
use std::thread;
use std::time::Duration;

#[path = "ai_repair_tests.rs"]
mod repair_tests;

fn config(base_url: String, key: &str) -> AiConfig {
    AiConfig { base_url, model: "mock-model".into(), api_key: key.into() }
}

fn nodes() -> Vec<Value> {
    vec![
        json!({"typeId":"text_input","execution_domain":"synchronous"}),
        json!({"typeId":"gain","execution_domain":"synchronous"}),
        json!({"typeId":"wav_input","execution_domain":"synchronous"}),
        json!({"typeId":"wav_output","execution_domain":"synchronous"}),
        json!({"typeId":"stream_gain","execution_domain":"streaming"}),
    ]
}

fn response(arguments: Option<&str>, name: &str) -> Value {
    let mut message = json!({"content":"模型说明"});
    if let Some(arguments) = arguments {
        message["tool_calls"] = json!([{"id":"call-1","type":"function","function":{
            "name":name,"arguments":arguments}}]);
    }
    json!({"choices":[{"finish_reason":"stop","message":message}],
        "usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3,"ignored":"x"}})
}

fn proposal(node_type: &str, extra: &str) -> String {
    format!(r#"{{"mode":"offline","graph":{{"schema_version":1,"nodes":[{{"id":"n","type":"{node_type}"}}],"connections":[]}}{extra}}}"#)
}

struct MockHttp {
    base_url: String,
    stop: Arc<AtomicBool>,
    request: mpsc::Receiver<Vec<u8>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl MockHttp {
    fn once(raw_response: Vec<u8>, hold: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, request) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = stop.clone();
        let thread = thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            let mut stream = loop {
                if stop_for_thread.load(Ordering::Acquire) { return; }
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && std::time::Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => return,
                }
            };
            // Keep the accepted stream blocking even if the platform inherits listener flags.
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let count = match stream.read(&mut buffer) {
                    Ok(count) => count,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                };
                if count == 0 { break; }
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    let length = headers.lines().find_map(|line| line.to_ascii_lowercase()
                        .strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok())).unwrap_or(0);
                    if bytes.len() >= end + 4 + length { break; }
                }
            }
            let _ = sender.send(bytes);
            if !hold.is_zero() { thread::sleep(hold); }
            let _ = stream.write_all(&raw_response);
        });
        Self { base_url: format!("http://{address}/v1"), stop, request, thread: Some(thread) }
    }
}
impl Drop for MockHttp {
    fn drop(&mut self) {
        // Stop only this fixture; connecting to a released ephemeral port can hit another test's listener.
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() { let _ = thread.join(); }
    }
}

#[test]
fn endpoint_policy_is_https_or_loopback_http_and_never_accepts_ambiguous_targets() {
    assert_eq!(endpoint_url(&config("https://example.test/v1/".into(), "")).unwrap().path(), "/v1/chat/completions");
    for url in ["", "http://example.test/v1", "file:///tmp/v1", "https://u:p@example.test/v1",
        "https://example.test/v1?q=x", "https://example.test/v1#x", "http://127.0.0.1:0/v1",
        "https://example.test/v1/chat/completions"] {
        assert!(endpoint_url(&config(url.into(), "")).is_err(), "accepted {url}");
    }
}

#[test]
fn generation_accepts_clarification_and_one_valid_tool_only() {
    let clarification = parse_generation(response(None, ""), "r1".into(), &nodes()).unwrap();
    assert!(clarification.proposal.is_none());
    let valid = proposal("text_input", "");
    let parsed = parse_generation(response(Some(&valid), "propose_audio_graph"), "r2".into(), &nodes()).unwrap();
    assert_eq!(parsed.proposal.unwrap().mode, "offline");
    assert_eq!(parsed.usage.unwrap()["total_tokens"], 3);
}

#[test]
fn malformed_duplicate_multiple_unknown_and_disallowed_proposals_are_rejected() {
    let cases = vec!["not json".to_owned(), r#"{"mode":"offline","mode":"streaming","graph":{}}"#.to_owned(),
        proposal("unknown_node", ""), proposal("text_input", r#", "script":"bad"#),
        proposal("text_input", r#", "options":{"block_frames":64}"#)];
    for arguments in cases {
        assert!(parse_generation(response(Some(&arguments), "propose_audio_graph"), "r".into(), &nodes()).is_err());
    }
    assert!(parse_generation(response(Some(&proposal("text_input", "")), "unknown"), "r".into(), &nodes()).is_err());
    let mut multiple = response(Some(&proposal("text_input", "")), "propose_audio_graph");
    let call = multiple.pointer("/choices/0/message/tool_calls/0").unwrap().clone();
    multiple.pointer_mut("/choices/0/message/tool_calls").unwrap().as_array_mut().unwrap().push(call);
    assert!(parse_generation(multiple, "r".into(), &nodes()).is_err());
}

#[test]
fn summary_is_plain_text_and_never_accepts_tools() {
    let parsed = parse_summary(response(None, ""), "summary".into()).unwrap();
    assert_eq!(parsed.text, "模型说明");
    assert!(parse_summary(response(Some("{}"), "propose_audio_graph"), "summary".into()).is_err());
    let proposal = AiProposal { mode: "offline".into(), graph: json!({"schema_version":1,
        "nodes":[{"id":"n","type":"text_input"}],"connections":[]}), options: json!({}) };
    let body = build_summary_body("m", "explain", &proposal, &json!({"state":"succeeded"})).unwrap();
    assert!(body.get("tools").is_none());
}

#[test]
fn local_http_sends_key_only_in_authorization_and_does_not_echo_it_on_error() {
    let server = MockHttp::once(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 20\r\n\r\ndummy-secret body!!".to_vec(), Duration::ZERO);
    let result = tauri::async_runtime::block_on(request_completion_with_timeout(
        &config(server.base_url.clone(), "dummy-secret"), json!({"model":"m"}), Duration::from_secs(2)));
    let error = result.unwrap_err();
    assert!(!error.contains("dummy-secret"));
    let request = server.request.recv_timeout(Duration::from_secs(2)).unwrap();
    let request = String::from_utf8_lossy(&request);
    assert!(request.to_ascii_lowercase().contains("authorization: bearer dummy-secret"));
    assert!(!request.split("\r\n\r\n").nth(1).unwrap_or("").contains("dummy-secret"));
}

#[test]
fn redirects_large_responses_and_timeouts_are_bounded_without_retry() {
    let oversized_config = config("http://127.0.0.1:9/v1".into(), "dummy");
    let oversized = request_completion_with_timeout(&oversized_config,
        json!({"padding":"x".repeat(MAX_REQUEST_BYTES)}), Duration::from_millis(20));
    assert!(tauri::async_runtime::block_on(oversized).unwrap_err().contains("128KiB"));
    for raw in [
        b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/steal\r\nContent-Length: 0\r\n\r\n".to_vec(),
        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", MAX_RESPONSE_BYTES + 1).into_bytes(),
    ] {
        let server = MockHttp::once(raw, Duration::ZERO);
        let result = tauri::async_runtime::block_on(request_completion_with_timeout(
            &config(server.base_url.clone(), "dummy"), json!({}), Duration::from_secs(2)));
        assert!(result.is_err());
        assert!(server.request.recv_timeout(Duration::from_secs(2)).is_ok());
    }
    let server = MockHttp::once(Vec::new(), Duration::from_millis(150));
    let result = tauri::async_runtime::block_on(request_completion_with_timeout(
        &config(server.base_url.clone(), "dummy"), json!({}), Duration::from_millis(20)));
    assert!(result.unwrap_err().contains("超时"));
}

#[test]
fn manager_timeout_and_exact_id_cancel_release_the_single_active_slot() {
    let server = MockHttp::once(Vec::new(), Duration::from_millis(200));
    let http_config = config(server.base_url.clone(), "dummy");
    tauri::async_runtime::block_on(async {
        let manager = std::sync::Arc::new(AiManager::default());
        let running = manager.clone();
        let handle = tokio::spawn(async move {
            running.run_with_timeout("active".into(),
                request_completion_with_timeout(&http_config, json!({}), Duration::from_secs(1)),
                Duration::from_secs(1)).await
        });
        assert!(server.request.recv_timeout(Duration::from_secs(2)).is_ok(), "HTTP request never reached fixture");
        assert!(!manager.cancel("old"));
        assert!(manager.cancel("active"));
        assert!(handle.await.unwrap().unwrap_err().contains("取消"));
        let timeout = manager.run_with_timeout("next".into(), async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok::<_, String>(())
        }, Duration::from_millis(5)).await.unwrap_err();
        assert!(timeout.contains("45秒"));
    });
}

#[cfg(windows)]
fn control_executable() -> std::path::PathBuf {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for candidate in [root.join("build/Debug/control-cli.exe"), root.join("build/Release/control-cli.exe")] {
        if candidate.is_file() { return std::fs::canonicalize(candidate).unwrap(); }
    }
    panic!("Build the C++ control-cli target before running the AI integration test");
}

#[cfg(windows)]
struct AiE2eFixture {
    backend: Option<Arc<crate::backend::BackendManager>>,
    root: std::path::PathBuf,
}

#[cfg(windows)]
impl AiE2eFixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(1);
        let root = loop {
            let candidate = std::env::temp_dir().join(format!("audioprocess_ai_e2e_{}_{}",
                std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            match std::fs::create_dir(&candidate) {
                Ok(()) => break std::fs::canonicalize(candidate).unwrap(),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("cannot create AI test directory: {error}"),
            }
        };
        let callback: crate::backend::DisconnectCallback = Arc::new(|_| {});
        let connection = crate::backend::Connection::spawn(&control_executable(), root.clone(),
            "ai-e2e-session".into(), false, false, callback).unwrap();
        let backend = Arc::new(crate::backend::BackendManager::with_test_connection(connection));
        Self { backend: Some(backend), root }
    }
    fn backend(&self) -> Arc<crate::backend::BackendManager> { self.backend.as_ref().unwrap().clone() }
}

#[cfg(windows)]
impl Drop for AiE2eFixture {
    fn drop(&mut self) {
        // 后台持有cwd和文件句柄；必须确认其退出后才清理唯一创建的目录。
        if let Some(backend) = self.backend.take() {
            let _ = backend.shutdown();
            drop(backend);
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[cfg(windows)]
fn write_pcm16_wav(path: &std::path::Path, samples: &[i16]) {
    let data_bytes = (samples.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + data_bytes as usize);
    bytes.extend_from_slice(b"RIFF"); bytes.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt "); bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes()); bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes()); bytes.extend_from_slice(&96_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes()); bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data"); bytes.extend_from_slice(&data_bytes.to_le_bytes());
    for sample in samples { bytes.extend_from_slice(&sample.to_le_bytes()); }
    std::fs::write(path, bytes).unwrap();
}

#[cfg(windows)]
fn read_first_pcm16(path: &std::path::Path) -> i16 {
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[0..4], b"RIFF");
    i16::from_le_bytes([bytes[44], bytes[45]])
}

fn http_json(value: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(value).unwrap();
    let mut response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
    response.extend_from_slice(&body);
    response
}

#[cfg(windows)]
#[test]
fn mock_model_validates_without_writing_then_explicit_approval_runs_real_wav_and_summary_has_no_tools() {
    let fixture = AiE2eFixture::new();
    write_pcm16_wav(&fixture.root.join("input.wav"), &[8192, 8192, 8192, 8192]); // PCM16 0.25
    let graph = json!({"schema_version":1,"nodes":[
        {"id":"input","type":"wav_input","parameters":{"path":"input.wav"}},
        {"id":"gain","type":"gain","parameters":{"gain_db":-6.020599913}},
        {"id":"output","type":"wav_output","parameters":{"path":"output.wav"}}
    ],"connections":[
        {"from":{"node":"input","port":"audio"},"to":{"node":"gain","port":"audio"}},
        {"from":{"node":"gain","port":"audio"},"to":{"node":"output","port":"audio"}}
    ],"exports":[{"name":"file","node":"output","port":"path"}]});
    let arguments = serde_json::to_string(&json!({"mode":"offline","graph":graph,"options":{}})).unwrap();
    let generate_server = MockHttp::once(http_json(&response(Some(&arguments), "propose_audio_graph")), Duration::ZERO);
    let generated = tauri::async_runtime::block_on(crate::ai_commands::generate_impl(
        Arc::new(AiManager::default()), fixture.backend(), "ai-e2e-session".into(), "generate-1".into(),
        config(generate_server.base_url.clone(), "dummy-only"), "halve the selected WAV into output.wav".into(),
        Some("input.wav".into()))).unwrap();
    assert_eq!(generated.inspection.as_ref().unwrap()["sample_rate"], 48000);
    let proposal = generated.proposal.unwrap();
    assert!(!fixture.root.join("output.wav").exists(), "generation and graph.validate must remain read-only");
    let generation_http = String::from_utf8_lossy(&generate_server.request.recv_timeout(Duration::from_secs(2)).unwrap()).into_owned();
    let generation_body: Value = serde_json::from_str(generation_http.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(generation_body["tools"].as_array().unwrap().len(), 1);
    let system = generation_body["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("propose_audio_graph") && system.contains("input_audio"));
    let user: Value = serde_json::from_str(generation_body["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(user["input_audio"]["sample_rate"], 48000);
    assert_eq!(user["input_audio"]["channels"], 1);
    assert_eq!(user["input_audio"]["frame_count"], 4);
    assert_eq!(user["input_audio"]["path"], "input.wav");

    // 这一步代表用户在UI中明确点击确认；此前没有tasks.start。
    let started = fixture.backend().request("ai-e2e-session", json!({"op":"tasks.start",
        "mode":proposal.mode,"graph":proposal.graph,"options":proposal.options})).unwrap();
    assert_eq!(started["success"], true);
    let task_id = started["data"]["task_id"].as_str().unwrap().to_owned();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        let status = fixture.backend().request("ai-e2e-session", json!({"op":"tasks.status","task_id":task_id})).unwrap();
        if status["data"]["state"] == "succeeded" { break; }
        assert!(std::time::Instant::now() < deadline, "real WAV task did not finish");
        thread::sleep(Duration::from_millis(2));
    }
    let result = fixture.backend().request("ai-e2e-session", json!({"op":"tasks.result","task_id":task_id})).unwrap();
    assert!(fixture.root.join("output.wav").is_file());
    assert!((read_first_pcm16(&fixture.root.join("output.wav")) - 4096).abs() <= 1, "0.25 sample was not reduced to 0.125");

    let summary_response = json!({"choices":[{"finish_reason":"stop","message":{"content":"真实任务成功，输出约减半。"}}]});
    let summary_server = MockHttp::once(http_json(&summary_response), Duration::ZERO);
    let summary = tauri::async_runtime::block_on(crate::ai_commands::summarize_impl(
        Arc::new(AiManager::default()), fixture.backend(), "ai-e2e-session".into(), "summary-1".into(),
        config(summary_server.base_url.clone(), "dummy-only"), "halve input.wav".into(), proposal,
        result["data"].clone())).unwrap();
    assert!(summary.text.contains("成功"));
    let summary_http = String::from_utf8_lossy(&summary_server.request.recv_timeout(Duration::from_secs(2)).unwrap()).into_owned();
    let summary_body: Value = serde_json::from_str(summary_http.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert!(summary_body.get("tools").is_none(), "summary must be an independent no-tools request");
}

#[cfg(windows)]
#[test]
fn explicit_input_inspection_failure_short_circuits_before_model_http() {
    let fixture = AiE2eFixture::new();
    write_pcm16_wav(&fixture.root.join("mentioned-in-prompt.wav"), &[1, 2, 3, 4]);
    let server = MockHttp::once(http_json(&response(None, "")), Duration::ZERO);
    let result = tauri::async_runtime::block_on(crate::ai_commands::generate_impl(
        Arc::new(AiManager::default()), fixture.backend(), "ai-e2e-session".into(), "inspect-fail".into(),
        config(server.base_url.clone(), "dummy-only"),
        "Use mentioned-in-prompt.wav, but the explicit selector is authoritative.".into(),
        Some("missing-explicit.wav".into())));
    assert!(result.err().expect("missing explicit input unexpectedly succeeded").contains("未发送模型请求"));
    assert!(server.request.recv_timeout(Duration::from_millis(100)).is_err(),
        "failed explicit inspection still contacted the model or guessed a path from prompt text");
}

/// Paid integration probe: never part of normal cargo test, never retries.
/// Only use with explicit approval. Read credentials from a closed stdin pipe, not files or argv.
/// Provider-specific cost controls below differ from the default desktop request.
#[cfg(windows)]
#[test]
#[ignore = "requires explicit approval and credential on stdin; makes at most two paid requests"]
fn live_deepseek_bounded_graph_roundtrip() {
    assert_eq!(std::env::var("AUDIOPROCESS_LIVE_APPROVED").as_deref(), Ok("yes"), "live test not approved");
    // Not an environment variable: the C++ sidecar must not inherit a model credential.
    let mut api_key = String::new();
    std::io::stdin().lock().take(4097).read_to_string(&mut api_key).expect("cannot read live credential");
    assert!(api_key.len() <= 4096, "live credential too long");
    assert!(!api_key.trim().is_empty(), "live credential empty");
    let config = AiConfig { base_url: "https://api.deepseek.com".into(), model: "deepseek-flash".into(), api_key };
    let fixture = AiE2eFixture::new();
    write_pcm16_wav(&fixture.root.join("input.wav"), &[8192; 4800]);
    let catalog = fixture.backend().request("ai-e2e-session", json!({"op":"nodes.list"})).unwrap();
    let nodes = catalog_nodes(&catalog).unwrap();
    let prompt = "用整段离线模式读取 input.wav，降低 6.020599913 dB，保存到新的 output.wav。请生成 wav_input → gain → wav_output 三节点图，导出输出文件路径。无需额外处理，说明尽量简短。";
    let mut body = build_generate_body(&config.model, prompt, &nodes, None).unwrap();
    body["max_tokens"] = json!(1536);
    body["thinking"] = json!({"type":"disabled"});
    println!("LIVE generation: one request, max output 1536, thinking disabled; no retry");
    let response = tauri::async_runtime::block_on(request_completion(&config, body)).unwrap();
    let generated = parse_generation(response, "live-generate".into(), &nodes).unwrap();
    println!("LIVE generation usage: {}", generated.usage.unwrap_or(Value::Null));
    let proposal = generated.proposal.expect("model returned no tool proposal; stopping without retry");
    assert_eq!(proposal.mode, "offline");
    let graph_nodes = proposal.graph["nodes"].as_array().unwrap();
    assert_eq!(graph_nodes.len(), 3, "unexpected plan; do not execute");
    for kind in ["wav_input", "gain", "wav_output"] {
        assert_eq!(graph_nodes.iter().filter(|node| node["type"] == kind).count(), 1, "unexpected plan; do not execute");
    }
    for node in graph_nodes {
        match node["type"].as_str().unwrap() {
            "wav_input" => assert_eq!(node["parameters"]["path"], "input.wav"),
            "wav_output" => assert_eq!(node["parameters"]["path"], "output.wav"),
            "gain" => assert!((node["parameters"]["gain_db"].as_f64().unwrap() + 6.020599913).abs() < 0.001),
            _ => panic!("unexpected node; do not execute"),
        }
    }
    let validation = fixture.backend().request("ai-e2e-session", json!({"op":"graph.validate",
        "mode":proposal.mode,"graph":proposal.graph,"options":proposal.options})).unwrap();
    assert_eq!(validation["success"], true, "live proposal failed C++ validation");
    assert!(!fixture.root.join("output.wav").exists());
    println!("LIVE proposal: registered nodes and C++ validation passed; no output before explicit test approval");
    // User-approved test, further restricted above to synthetic input and one new output.
    let started = fixture.backend().request("ai-e2e-session", json!({"op":"tasks.start",
        "mode":proposal.mode,"graph":proposal.graph,"options":proposal.options})).unwrap();
    assert_eq!(started["success"], true);
    let task_id = started["data"]["task_id"].as_str().unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        let status = fixture.backend().request("ai-e2e-session", json!({"op":"tasks.status","task_id":task_id})).unwrap();
        assert_eq!(status["success"], true);
        let state = status["data"]["state"].as_str().unwrap();
        if state == "succeeded" { break; }
        assert!(matches!(state, "queued" | "running"), "live audio task failed");
        assert!(std::time::Instant::now() < deadline, "live audio task timed out");
        thread::sleep(Duration::from_millis(10));
    }
    let result = fixture.backend().request("ai-e2e-session", json!({"op":"tasks.result","task_id":task_id})).unwrap();
    assert_eq!(result["success"], true);
    let output = std::fs::read(fixture.root.join("output.wav")).unwrap();
    assert_eq!(output.len(), 44 + 4800 * 2);
    assert!(output[44..].chunks_exact(2).all(|sample|
        (i16::from_le_bytes([sample[0], sample[1]]) as i32 - 4096).abs() <= 1));
    println!("LIVE C++ output verified: 4800 frames, all PCM16 samples approximately 4096 (input 8192)");
    let mut summary_body = build_summary_body(&config.model, prompt, &proposal, &result["data"]).unwrap();
    assert!(summary_body.get("tools").is_none());
    summary_body["max_tokens"] = json!(256);
    summary_body["thinking"] = json!({"type":"disabled"});
    println!("LIVE summary: one request, max output 256, no tools, no retry");
    let response = tauri::async_runtime::block_on(request_completion(&config, summary_body)).unwrap();
    let summary = parse_summary(response, "live-summary".into()).unwrap();
    assert!(!summary.text.is_empty());
    println!("LIVE summary usage: {}", summary.usage.unwrap_or(Value::Null));
    println!("LIVE PASS: two requests; no audio uploaded; credentials not persisted");
}
