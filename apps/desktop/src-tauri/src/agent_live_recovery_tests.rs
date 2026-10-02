//! Paid, opt-in runtime repair regression. Reports deliberately omit model text and configuration.
use super::*;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

fn wav(rate: u32, channels: u16) -> Vec<u8> {
    let frames = rate / 5;
    let size = frames * u32::from(channels) * 2;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF"); bytes.extend_from_slice(&(36 + size).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt "); bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes()); bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
    bytes.extend_from_slice(&(channels * 2).to_le_bytes()); bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data"); bytes.extend_from_slice(&size.to_le_bytes());
    for frame in 0..frames { for _ in 0..channels {
        let sample: i16 = if frame % 48 < 24 { 6000 } else { -6000 };
        bytes.extend_from_slice(&sample.to_le_bytes());
    }}
    bytes
}

fn graph(input: &str, output: &str) -> Value {
    json!({"schema_version":1,"nodes":[
        {"id":"input","type":"wav_input","parameters":{"path":input}},
        {"id":"denoise","type":"rnnoise_denoise"},
        {"id":"output","type":"wav_output","parameters":{"path":output}}
    ],"connections":[
        {"from":{"node":"input","port":"audio"},"to":{"node":"denoise","port":"audio"}},
        {"from":{"node":"denoise","port":"audio"},"to":{"node":"output","port":"audio"}}
    ],"exports":[{"name":"file","node":"output","port":"path"}]})
}

fn seed(spaces: &ToolWorkspaces) -> (Vec<u8>, Vec<u8>, String) {
    std::fs::create_dir_all(&spaces.ai_root).unwrap();
    let good = wav(48000, 1); let bad = wav(44100, 2);
    for (name, bytes) in [("normal.wav", &good), ("legacy-stereo.wav", &bad)] {
        std::fs::write(spaces.user_root.join(name), bytes).unwrap();
        std::fs::write(spaces.ai_root.join(name), bytes).unwrap();
    }
    let document = json!({"schema_version":1,"inputs":{},"steps":[
        {"id":"good","type":"call","tool":"graph_run","args":{"mode":"offline","graph":graph("normal.wav","good.wav")}},
        {"id":"bad","type":"call","tool":"graph_run","args":{"mode":"offline","graph":graph("legacy-stereo.wav","bad.wav")}}
    ],"outputs":{"good":{"$ref":"/steps/good"},"bad":{"$ref":"/steps/bad"}}});
    let text = document.to_string();
    std::fs::write(spaces.ai_root.join("legacy.workflow.json"), &text).unwrap();
    (good, bad, text)
}

fn has_code(value: &Value, code: &str) -> bool {
    match value {
        Value::Object(object) => object.get("code").and_then(Value::as_str) == Some(code)
            || object.values().any(|v| has_code(v, code)),
        Value::Array(array) => array.iter().any(|v| has_code(v, code)),
        _ => false,
    }
}

fn valid_delivery(path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else { return false; };
    if bytes.len() < 44 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" { return false; }
    let mut cursor = 12; let mut format = false; let mut data = false;
    while cursor + 8 <= bytes.len() {
        let length = u32::from_le_bytes(bytes[cursor+4..cursor+8].try_into().unwrap()) as usize;
        let start = cursor + 8;
        let Some(end) = start.checked_add(length) else { return false; };
        if end > bytes.len() { return false; }
        if &bytes[cursor..cursor+4] == b"fmt " && length >= 16 {
            format = bytes[start..start+4] == [1,0,1,0]
                && u32::from_le_bytes(bytes[start+4..start+8].try_into().unwrap()) == 48000
                && bytes[start+14..start+16] == [16,0];
        }
        if &bytes[cursor..cursor+4] == b"data" { data = length > 0 && length % 2 == 0; }
        cursor = end + length % 2;
    }
    format && data
}

fn file_hash(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|bytes| format!("{:x}", Sha256::digest(bytes)))
}

#[test]
fn recovery_fixture_fails_in_real_cpp_after_successful_first_step() {
    tauri::async_runtime::block_on(async {
        super::end_to_end::ensure_test_sidecar();
        let (root, spaces, backend) = test_backend(); seed(&spaces);
        let store = Arc::new(RunStore::default());
        let mut tools = ToolContext::new(spaces.clone(), backend.clone(), "script-user".into())
            .with_records(store.clone(), root.join("data"));
        let report = tools.dispatch("workflow", "workflow_run", &json!({"space":"ai","path":"legacy.workflow.json"}), &AtomicBool::new(false)).await.unwrap();
        assert_eq!(report["state"], "failed");
        assert!(report["step_results"].get("good").is_some());
        assert!(report["step_results"].get("bad").is_none());
        let records = store.list(&spaces, &root.join("data")).unwrap().records;
        assert!(records.iter().any(|r| {
            let record = store.load(&spaces, &root.join("data"), &r.id).unwrap();
            record.kind == "graph" && record.state == "failed"
                && record.result.as_ref().is_some_and(|v| has_code(v, "unsupported_audio_format"))
        }), "missing structured C++ format error");
        assert!(valid_delivery(&spaces.ai_root.join("good.wav")));
        backend.shutdown().unwrap();
        // Retain this unique synthetic fixture; never clean actual application data.
    });
}

#[cfg(windows)]
#[test]
#[ignore = "paid real model recovery: explicit approval and closed stdin AiConfig required"]
fn live_workflow_runtime_failure_repair() {
    assert_eq!(std::env::var("AUDIOPROCESS_LIVE_APPROVED").as_deref(), Ok("yes"));
    let mut input = String::new();
    std::io::stdin().lock().take(16 * 1024 + 1).read_to_string(&mut input).expect("cannot read live config");
    assert!(input.len() <= 16 * 1024, "live config exceeds limit");
    let config: AiConfig = serde_json::from_str(&input).unwrap_or_else(|_| panic!("invalid live config"));
    drop(input);
    tauri::async_runtime::block_on(async {
        super::end_to_end::ensure_test_sidecar();
        let (root, spaces, backend) = test_backend();
        let (original_good, original_bad, legacy) = seed(&spaces);
        let report_dir = std::env::var_os("AUDIOPROCESS_LIVE_REPORT_DIR").map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../out")
                .join(format!("live-workflow-recovery-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())));
        std::fs::create_dir_all(report_dir.parent().unwrap()).unwrap();
        std::fs::create_dir(&report_dir).expect("report directory must be fresh");
        std::fs::write(report_dir.join("seeded.workflow.json"), &legacy).unwrap();
        let manager = AgentManager::default(); let store = Arc::new(RunStore::default());
        let (cancel, lease) = manager.begin("live-workflow-recovery").unwrap();
        let deadline = cancel.clone();
        let timer = tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(600)).await; deadline.store(true, Ordering::Release);
        });
        let prompt = "这是仅含合成音频的隔离回归测试。AI空间已准备 legacy.workflow.json 和两个输入。首先必须原样运行该 Workflow 文件一次以复现旧流程，不得提前修改、替换、跳过或通过预处理绕过复现；可先读取或纯校验。然后根据实际工具失败反馈并读取运行历史，保留已经成功生成的 good.wav（不得重跑成功输入或覆盖它），只修复失败输入的格式兼容问题：自行发现节点能力，编写并校验修订的 Workflow JSON 文件，再运行该文件完成失败部分。不要仅直接运行Graph。把已有 good.wav 与修复后的音频通过 file_export 交付到新用户子目录 recovery-delivery/good.wav 与 recovery-delivery/bad.wav，检查交付元数据。用户空间两个原始文件必须完全保留。不要调用HTTP或写脚本。尽量合并独立工具调用，在16次模型回复和40次工具调用内完成；最终简短说明实际结果。";
        println!("LIVE recovery: synthetic inputs, production limits {} model / {} tools, 600s cooperative deadline", MAX_MODEL_CALLS, MAX_TOOL_CALLS);
        let response = run_turn(&manager, Arc::new(AiManager::default()), backend.clone(), spaces.clone(),
            "script-user".into(), "workflow".into(), "live-workflow-recovery".into(), prompt.into(),
            config, None, cancel, Some((store.clone(),root.join("data"))), None, None).await;
        timer.abort(); drop(lease);
        let statuses: Vec<Value> = response.events.iter().filter_map(|event| event.tool.as_ref().map(|name| {
            println!("LIVE tool={} success={}",name,event.success.unwrap_or(false));
            json!({"tool":name,"success":event.success})
        })).collect();
        let workflows: Vec<_> = response.events.iter().filter(|e| e.tool.as_deref() == Some("workflow_run")).collect();
        let records: Vec<_> = store.list(&spaces,&root.join("data")).unwrap().records.into_iter()
            .map(|r| store.load(&spaces,&root.join("data"),&r.id).unwrap()).collect();
        let graphs: Vec<_> = records.iter().filter(|r| r.kind == "graph").collect();
        let seeded_sha = format!("{:x}",Sha256::digest(legacy.as_bytes()));
        let reproduction = workflows.first().is_some_and(|e| e.success == Some(false)
            && e.result.as_ref().is_some_and(|r| r["data"]["state"] == "failed"
                && r.pointer("/data/source/sha256").and_then(Value::as_str) == Some(seeded_sha.as_str())
                && r.pointer("/data/step_results/good").is_some()
                && r.pointer("/data/step_results/bad").is_none()));
        let cpp_failure = graphs.iter().any(|r| r.state == "failed"
            && r.result.as_ref().is_some_and(|v| has_code(v,"unsupported_audio_format")));
        let good_graphs: Vec<_> = graphs.iter().filter(|r| r.configuration["graph"]["nodes"].as_array().is_some_and(|nodes|
            nodes.iter().any(|n| n["type"] == "wav_input" && n["parameters"]["path"] == "normal.wav"))).collect();
        let preserved_good = good_graphs.len() == 1 && good_graphs[0].state == "succeeded"
            && good_graphs[0].files.iter().any(|f| f.role == "output" && f.path == "good.wav"
                && f.sha256.as_deref() == std::fs::read(spaces.ai_root.join("good.wav")).ok()
                    .map(|b| format!("{:x}",Sha256::digest(b))).as_deref());
        let revised_ids: Vec<&str> = workflows.iter().skip(1).filter(|e| e.success == Some(true))
            .filter_map(|e| e.result.as_ref()).filter(|r|
                r.pointer("/data/source/sha256").and_then(Value::as_str).is_some_and(|sha| sha != seeded_sha))
            .filter_map(|r| r.pointer("/data/run_id").and_then(Value::as_str)).collect();
        let repaired_graph = graphs.iter().any(|r| r.state == "succeeded"
            && r.parent_id.as_deref().is_some_and(|id| revised_ids.contains(&id))
            && r.configuration["graph"]["nodes"].as_array().is_some_and(|nodes|
                nodes.iter().any(|n| n["type"] == "wav_input" && n["parameters"]["path"] == "legacy-stereo.wav")
                && nodes.iter().any(|n| n["type"] == "rnnoise_denoise")
                && nodes.iter().any(|n| n["type"] == "audio_downmix_mono")
                && nodes.iter().any(|n| n["type"] == "audio_resample"))
            && r.files.iter().any(|f| f.role == "output" && f.sha256.is_some()
                && f.sha256 == file_hash(&spaces.user_root.join("recovery-delivery/bad.wav"))));
        let originals_preserved = std::fs::read(spaces.user_root.join("normal.wav")).unwrap() == original_good
            && std::fs::read(spaces.user_root.join("legacy-stereo.wav")).unwrap() == original_bad;
        let delivered = ["good.wav","bad.wav"].iter().all(|name| valid_delivery(&spaces.user_root.join("recovery-delivery").join(name)))
            && file_hash(&spaces.ai_root.join("good.wav")) == file_hash(&spaces.user_root.join("recovery-delivery/good.wav"));
        let history = response.events.iter().any(|e| matches!(e.tool.as_deref(),Some("runs_read" | "runs_list")) && e.success == Some(true));
        let successful = |name: &str| response.events.iter().filter(|e| e.tool.as_deref() == Some(name) && e.success == Some(true)).count();
        let validated_revision = workflows.iter().skip(1).filter(|e| e.success == Some(true))
            .filter_map(|e| e.result.as_ref()).any(|r| {
                let sha = r.pointer("/data/source/sha256");
                sha.is_some() && sha.and_then(Value::as_str) != Some(seeded_sha.as_str())
                    && response.events.iter().filter(|e| e.tool.as_deref() == Some("workflow_validate") && e.success == Some(true))
                        .filter_map(|e| e.result.as_ref()).any(|v| v.pointer("/data/source/sha256") == sha)
            });
        let discovery_and_delivery_tools = successful("nodes_list") > 0
            && successful("file_export") >= 2 && successful("audio_inspect") >= 2;
        let summary = json!({"state":response.state,"model_calls":response.model_calls,"tool_calls":response.tool_calls,
            "production_model_limit":MAX_MODEL_CALLS,"production_tool_limit":MAX_TOOL_CALLS,
            "tool_statuses":statuses,"reproduced_original_failure":reproduction,"structured_cpp_format_error":cpp_failure,
            "successful_input_preserved":preserved_good,"repaired_graph_with_format_nodes":repaired_graph,
            "originals_preserved":originals_preserved,"valid_48k_mono_deliveries":delivered,"history_read":history,
            "validated_revision":validated_revision,"discovery_and_delivery_tools":discovery_and_delivery_tools,
            "runs":records.iter().map(|r| json!({"kind":r.kind,"state":r.state,"parent_id":r.parent_id,"id":r.id})).collect::<Vec<_>>()});
        std::fs::write(report_dir.join("report.json"),serde_json::to_vec_pretty(&summary).unwrap()).unwrap();
        // Persist only executed Workflow configurations from this isolated synthetic task.
        // Never persist provider configuration, message bodies, reasoning or HTTP traffic.
        for (index, record) in records.iter().filter(|r| r.kind == "workflow").enumerate() {
            if let Some(document) = record.configuration.get("workflow") {
                std::fs::write(report_dir.join(format!("executed-workflow-{index}.json")),
                    serde_json::to_vec_pretty(document).unwrap()).unwrap();
            }
        }
        for name in ["good.wav","bad.wav"] { let source = spaces.user_root.join("recovery-delivery").join(name);
            if valid_delivery(&source) { std::fs::copy(source,report_dir.join(name)).unwrap(); }
        }
        backend.shutdown().unwrap();
        println!("LIVE sanitized recovery report: {}",report_dir.display());
        assert!(reproduction,"MISSING COVERAGE: original Workflow was not run unchanged and failed after its successful first step");
        assert!(cpp_failure,"missing actual structured C++ unsupported_audio_format error");
        assert_eq!(response.state,"completed","turn incomplete; inspect sanitized report");
        assert!(workflows.iter().skip(1).any(|e| e.success == Some(true)),"no successful revised Workflow file");
        assert!(preserved_good,"first successful input rerun or output overwritten");
        assert!(repaired_graph,"missing successful Workflow child Graph format adaptation");
        assert!(validated_revision && discovery_and_delivery_tools,"revision validation, node discovery or delivery tools missing");
        assert!(originals_preserved && delivered && history,"source preservation, delivery or history evidence missing");
    });
}
