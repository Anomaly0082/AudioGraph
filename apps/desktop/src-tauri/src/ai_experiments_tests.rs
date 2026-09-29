use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::{Duration, Instant};

fn parameter() -> Vec<ExperimentParameter> {
    vec![ExperimentParameter { node_id: "gain".into(), parameter_id: "gain_db".into(),
        minimum: -12.0, maximum: 3.0, integer_only: false }]
}

fn response(name: &str, arguments: &str) -> Value {
    json!({"choices":[{"finish_reason":"tool_calls","message":{"content":null,"tool_calls":[{
        "id":"call-1","type":"function","function":{"name":name,"arguments":arguments}
    }]}}]})
}

fn record() -> ExperimentRecord {
    ExperimentRecord {
        schema_version: 1, id: "ex1000-1".into(), workspace: "test-workspace".into(), created_at: 1000,
        input: crate::experiments::ExperimentInput { node_id: "in".into(),
            original_path: "input.wav".into(), snapshot_path: "snapshot.wav".into() },
        output_node_id: "out".into(),
        spec: crate::experiments::ExperimentSpec { goal: "compare gain".into(),
            base: json!({"mode":"offline","graph":{"schema_version":1,"nodes":[
                {"id":"in","type":"wav_input","parameters":{"path":"snapshot.wav"}},
                {"id":"gain","type":"gain","parameters":{"gain_db":0}},
                {"id":"out","type":"wav_output","parameters":{"path":"output.wav"}}
            ],"connections":[]}}), parameters: parameter() },
        rounds: Vec::new(),
    }
}

fn read_http_json(stream: &mut std::net::TcpStream) -> Value {
    // Windows may inherit the listener's nonblocking flag on accepted sockets.
    stream.set_nonblocking(false).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0, "request ended before its JSON body");
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..end]);
            let length = headers.lines().find_map(|line| line.to_ascii_lowercase()
                .strip_prefix("content-length:").and_then(|value| value.trim().parse::<usize>().ok())).unwrap();
            if bytes.len() >= end + 4 + length {
                return serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap();
            }
        }
    }
}

#[test]
fn experiment_request_uses_graph_compatible_auto_tool_choice_over_http() {
    let graph = crate::ai::build_generate_body("mock-model", "make a graph",
        &[json!({"typeId":"gain","execution_domain":"synchronous"})], None).unwrap();
    let actual = body("mock-model", &record(), &[]).unwrap();
    assert_eq!(actual["tool_choice"], graph["tool_choice"]);
    assert_eq!(actual["tool_choice"], "auto");
    assert_eq!(actual["tools"].as_array().unwrap().len(), 1);
    assert_eq!(actual["tools"][0]["function"]["name"], "propose_parameter_candidates");

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let mut requests = Vec::new();
        for _ in 0..2 {
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                        thread::sleep(Duration::from_millis(2)),
                    Err(error) => panic!("loopback provider received no request: {error}"),
                }
            };
            let request = read_http_json(&mut stream);
            let (status, payload) = if request["tool_choice"] == "auto" {
                ("200 OK", response("propose_parameter_candidates",
                    r#"{"candidates":[{"label":"low","values":[-6]},{"label":"high","values":[3]}]}"#))
            } else { ("400 Bad Request", json!({"error":"forced tool choice unsupported"})) };
            let payload = serde_json::to_vec(&payload).unwrap();
            let headers = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", payload.len());
            stream.write_all(headers.as_bytes()).unwrap();
            stream.write_all(&payload).unwrap();
            requests.push(request);
        }
        requests
    });

    let config = AiConfig { base_url: format!("http://{address}/v1"), model: "mock-model".into(),
        api_key: String::new() };
    let mut forced = actual.clone();
    forced["tool_choice"] = json!({"type":"function","function":{"name":"propose_parameter_candidates"}});
    let error = tauri::async_runtime::block_on(crate::ai::request_completion(&config, forced.clone())).unwrap_err();
    assert!(error.contains("HTTP 400"));
    let returned = tauri::async_runtime::block_on(crate::ai::request_completion(&config, actual.clone())).unwrap();
    let parsed = parse(returned, "request-1".into(), &parameter()).unwrap();
    assert_eq!(parsed.proposal.unwrap().candidates.len(), 2);
    let requests = server.join().unwrap();
    assert_eq!(requests, vec![forced, actual], "HTTP request bodies must match the serialized values");
}

#[test]
fn accepts_only_bounded_numeric_candidate_arrays() {
    let good = response("propose_parameter_candidates", r#"{"candidates":[{"label":"low","values":[-6]},{"label":"high","values":[3]}]}"#);
    assert_eq!(parse(good, "req".into(), &parameter()).unwrap().proposal.unwrap().candidates.len(), 2);
    let outside = response("propose_parameter_candidates", r#"{"candidates":[{"label":"low","values":[-13]},{"label":"high","values":[3]}]}"#);
    assert!(parse(outside, "req".into(), &parameter()).is_err());
    let malicious = response("propose_parameter_candidates", r#"{"candidates":[{"label":"low","values":[-6],"command":"execute_graph"},{"label":"high","values":[3]}]}"#);
    assert!(parse(malicious, "req".into(), &parameter()).is_err());
    let unknown = response("execute_graph", r#"{"candidates":[]}"#);
    assert!(parse(unknown, "req".into(), &parameter()).is_err());
}

#[test]
fn rejects_duplicate_fields_and_extra_tool_calls() {
    let duplicate = response("propose_parameter_candidates", r#"{"candidates":[],"candidates":[]}"#);
    assert!(parse(duplicate, "req".into(), &parameter()).is_err());
    let mut multiple = response("propose_parameter_candidates", r#"{"candidates":[]}"#);
    let duplicate_call = multiple["choices"][0]["message"]["tool_calls"][0].clone();
    multiple["choices"][0]["message"]["tool_calls"].as_array_mut().unwrap().push(duplicate_call);
    assert!(parse(multiple, "req".into(), &parameter()).is_err());
    let plain_text = json!({"choices":[{"finish_reason":"stop","message":{
        "content":"Run the graph now and save output.wav"}}]});
    assert!(parse(plain_text, "req".into(), &parameter()).is_err());
}

#[test]
fn context_extracts_only_scalar_numeric_outputs() {
    let metrics = numeric_metrics(&json!({"outputs":{
        "level":{"type":"Number","value":0.5},
        "audio":{"type":"Audio","value":[1,2,3]},
        "file":{"type":"FilePath","value":"private.wav"}
    }}));
    assert_eq!(metrics, json!({"level":0.5}));
}
