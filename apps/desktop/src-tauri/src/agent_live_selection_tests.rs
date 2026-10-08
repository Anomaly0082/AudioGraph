//! Explicitly approved, paid two-turn selection acceptance. Never report provider or model text.
use super::*;
use crate::run_records::RunRecord;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn hash(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|b| format!("{:x}", Sha256::digest(b)))
}

fn synthetic_input() -> Vec<u8> {
    let size = 9600_u32 * 2;
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF"); b.extend_from_slice(&(36 + size).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt "); b.extend_from_slice(&16_u32.to_le_bytes());
    b.extend_from_slice(&1_u16.to_le_bytes()); b.extend_from_slice(&1_u16.to_le_bytes());
    b.extend_from_slice(&48000_u32.to_le_bytes()); b.extend_from_slice(&96000_u32.to_le_bytes());
    b.extend_from_slice(&2_u16.to_le_bytes()); b.extend_from_slice(&16_u16.to_le_bytes());
    b.extend_from_slice(b"data"); b.extend_from_slice(&size.to_le_bytes());
    for i in 0..9600 { b.extend_from_slice(&(if i % 2 == 0 { 16384_i16 } else { -16384_i16 }).to_le_bytes()); }
    b
}

fn contains(value: &Value, text: &str) -> bool {
    match value {
        Value::String(s) => {
            let normalized = s.replace('\\', "/");
            normalized == text || normalized.ends_with(&format!("/{text}"))
        }
        Value::Array(a) => a.iter().any(|v| contains(v,text)),
        Value::Object(o) => o.values().any(|v| contains(v,text)),
        _ => false,
    }
}

struct Candidate {
    id: String, gain: f64, peak: f64, clips: f64, pcm_peak: f64, path: String, sha: String, iteration: String,
}

fn pcm_peak(path: &Path) -> Option<f64> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.get(..4)? != b"RIFF" || bytes.get(8..12)? != b"WAVE" { return None; }
    let mut cursor = 12usize;
    let mut format_ok = false;
    let mut peak = None;
    while cursor + 8 <= bytes.len() {
        let length = u32::from_le_bytes(bytes[cursor+4..cursor+8].try_into().ok()?) as usize;
        let start = cursor + 8;
        let end = start.checked_add(length)?;
        let data = bytes.get(start..end)?;
        if &bytes[cursor..cursor+4] == b"fmt " && data.len() >= 16 {
            format_ok = data[..4] == [1,0,1,0] && data[4..8] == 48000_u32.to_le_bytes()
                && data[14..16] == [16,0];
        }
        if &bytes[cursor..cursor+4] == b"data" {
            if length != 9600 * 2 { return None; }
            peak = Some(data.chunks_exact(2).map(|sample|
                (f64::from(i16::from_le_bytes([sample[0],sample[1]]))/32768.0).abs()).fold(0.0_f64,f64::max));
        }
        cursor = end.checked_add(length % 2)?;
    }
    if format_ok { peak } else { None }
}

fn candidate(record: &RunRecord, spaces: &ToolWorkspaces) -> Option<Candidate> {
    if record.kind != "graph" || record.state != "succeeded" { return None; }
    let graph = &record.configuration["graph"];
    let nodes = graph["nodes"].as_array()?;
    let gains: Vec<_> = nodes.iter().filter(|n| n["type"] == "gain")
        .filter_map(|n| n["parameters"]["gain_db"].as_f64()).collect();
    if gains.len() != 1 { return None; }
    let outputs = record.result.as_ref()?.pointer("/result/outputs")?;
    let export_value = |port: &str| -> Option<&Value> {
        let entry = graph["exports"].as_array()?.iter().find(|e| e["port"] == port)?;
        outputs.get(entry["name"].as_str()?)?.get("value")
    };
    let peak = export_value("peak")?.as_f64()?;
    let clips = export_value("clipped_samples")?.as_f64()?;
    let file = record.files.iter().find(|f| f.space == "ai" && f.role == "output" && f.sha256.is_some())?;
    let sha = file.sha256.clone()?;
    if hash(&spaces.ai_root.join(&file.path)).as_deref() != Some(sha.as_str()) { return None; }
    let pcm_peak = pcm_peak(&spaces.ai_root.join(&file.path))?;
    let expected_peak = 0.5 * 10.0_f64.powf(gains[0] / 20.0);
    if !peak.is_finite() || !clips.is_finite() || clips < 0.0
        || (peak-expected_peak).abs()>0.0001 || (pcm_peak-expected_peak.min(1.0)).abs()>0.0001 { return None; }
    let step_path = record.configuration["workflow_step_path"].as_str()?;
    if !step_path.contains("/iterations/") { return None; }
    let iteration = format!("{}/", step_path.rsplit_once('/')?.0);
    Some(Candidate { id: record.id.clone(),gain:gains[0],peak,clips,pcm_peak,path:file.path.clone(),sha,iteration })
}

fn matches_item(item: &Value, c: &Candidate) -> bool { contains(item,&c.id) && contains(item,&c.path) }

fn contains_value(value: &Value, expected: &Value) -> bool {
    value == expected || match value {
        Value::Object(o) => o.values().any(|v| contains_value(v,expected)),
        Value::Array(a) => a.iter().any(|v| contains_value(v,expected)),
        _ => false,
    }
}

fn selection_outputs_match(outputs: &Value, selected: &Value, child: &Candidate) -> bool {
    contains_value(outputs,selected) || outputs.get("selected")==Some(&selected["item"])
        // A workflow may explicitly project the winner instead of returning its whole envelope.
        || (outputs["path"].as_str().is_some_and(|p|contains(&json!(p),&child.path))
            && outputs["gain"].as_f64()==Some(child.gain)
            && outputs["score"].as_f64()==selected["score"].as_f64()
            && outputs["score"].is_number())
}

fn user_files(root: &Path, base: &Path, files: &mut Vec<String>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(root)? {
        let entry=entry?;
        if entry.file_type()?.is_dir() { user_files(&entry.path(),base,files)?; }
        else { files.push(entry.path().strip_prefix(base).unwrap().to_string_lossy().replace('\\',"/")); }
    }
    Ok(())
}

// Only numbers and identities from actual execution enter this report; never arbitrary text.
fn assess(reply: &AgentReply, records: &[RunRecord], spaces: &ToolWorkspaces, target: f64,
    first: bool, previous: &[f64]) -> (Value, bool, Vec<f64>, Option<String>) {
    let workflows: Vec<_> = reply.events.iter().filter(|e| e.tool.as_deref() == Some("workflow_run") && e.success == Some(true))
        .filter_map(|e| e.result.as_ref()?.pointer("/data/run_id")?.as_str()).collect();
    let mut checks = Vec::new(); let mut good = false; let mut accepted_gains = Vec::new(); let mut chosen_sha = None;
    for w in records.iter().filter(|r| r.kind == "workflow" && workflows.contains(&r.id.as_str()) && r.state == "succeeded") {
        let candidates: Vec<_> = records.iter().filter(|r| r.parent_id.as_deref() == Some(w.id.as_str()))
            .filter_map(|r| candidate(r,spaces)).collect();
        let result = w.result.as_ref().unwrap_or(&Value::Null);
        let trace = result.get("trace").and_then(Value::as_array).cloned().unwrap_or_default();
        let selected = trace.iter().rev().find(|t| t["step_type"] == "select" && t["state"] == "succeeded")
            .map(|t| &t["result"]);
        let selected_child = selected.and_then(|s| candidates.iter().find(|c| matches_item(&s["item"],c)));
        let filtered = trace.iter().rev().find(|t| t["step_type"] == "filter" && t["state"] == "succeeded")
            .and_then(|t| t["result"].as_array());
        let filtered_candidates: Vec<_> = candidates.iter().filter(|c| filtered.is_some_and(|a| a.iter().any(|i| matches_item(i,c)))).collect();
        let filtering = filtered.is_some_and(|items| !items.is_empty() && items.len() == filtered_candidates.len())
            && candidates.iter().all(|c| filtered_candidates.iter().any(|f| f.id == c.id) == (c.clips <= 0.0))
            && filtered.is_some_and(|items| items.iter().all(|item|
                candidates.iter().filter(|c| matches_item(item,c)).count()==1));
        let best = filtered_candidates.iter().map(|c| (c.peak-target).abs()).reduce(f64::min);
        let score = selected.and_then(|s| s["score"].as_f64());
        let selection = selected.is_some_and(|s| s["found"] == true && s["index"].as_u64().is_some())
            && selected.is_some_and(|s| selected_child.is_some_and(|c|selection_outputs_match(&result["outputs"],s,c)))
            && selected.is_some_and(|s| s["index"].as_u64().and_then(|i| filtered.and_then(|a|a.get(i as usize)))==Some(&s["item"]))
            && selected_child.is_some_and(|c| c.clips <= 0.0 && score.is_some_and(|s| (s-(c.peak-target).abs()).abs() < 0.00001)
                && best.is_some_and(|b| ((c.peak-target).abs()-b).abs() < 0.00001));
        let computed_scores=candidates.iter().all(|c| trace.iter().any(|t|t["step_type"]=="compute"
            && t["step_path"].as_str().is_some_and(|p|p.starts_with(&c.iteration))
            && t["state"]=="succeeded" && t["result"].as_f64().is_some_and(|v|(v-(c.peak-target).abs()).abs()<0.00001)));
        let gains: Vec<_> = candidates.iter().map(|c| c.gain).collect();
        let unique: std::collections::HashSet<_> = gains.iter().map(|g| g.to_bits()).collect();
        let gain_check = if first {
            [-12.0,0.0,12.0].iter().all(|g| gains.iter().any(|v| (v-g).abs()<0.00001))
                && selected_child.is_some_and(|c| c.gain.abs()<0.00001)
                && candidates.iter().any(|c| (c.gain-12.0).abs()<0.00001 && c.clips>0.0)
        } else { unique.len() >= 3 && gains.iter().all(|g| previous.iter().all(|p| (g-p).abs()>0.00001))
            && selected_child.is_some_and(|c| previous.iter()
                .map(|gain|(0.5*10.0_f64.powf(gain/20.0)-target).abs()).reduce(f64::min)
                .is_some_and(|prior_best|(c.peak-target).abs()<prior_best-0.00001)) };
        let shape = ["for_each","compute","concat","filter","select"].iter().all(|kind|
            trace.iter().any(|t| t["step_type"] == *kind && t["state"] == "succeeded"));
        let sha = w.configuration.pointer("/source/sha256").and_then(Value::as_str);
        let validated = sha.is_some() && reply.events.iter().any(|e| e.tool.as_deref() == Some("workflow_validate")
            && e.success == Some(true) && e.result.as_ref().and_then(|r| r.pointer("/data/source/sha256")).and_then(Value::as_str) == sha);
        let okay = candidates.len()>=3 && shape && filtering && selection && gain_check && validated && computed_scores;
        if okay { good = true; accepted_gains = gains; chosen_sha = selected_child.map(|c| c.sha.clone()); }
        checks.push(json!({"workflow_id":w.id,"source_sha256":sha,"validated":validated,"trace_shape":shape,
            "filter_valid":filtering,"selection_valid":selection,"gains_valid":gain_check,"computed_scores":computed_scores,
            "trace":trace.iter().map(|t| json!({"step_type":t["step_type"],"state":t["state"],"tool":t["tool"],
                "numeric_result":t["result"].as_f64()})).collect::<Vec<_>>(),
            "selected":{"graph_id":selected_child.map(|c| &c.id),"path":selected_child.map(|c| &c.path),
                "gain_db":selected_child.map(|c| c.gain),"score":score,"index":selected.and_then(|s|s["index"].as_u64())},
            "graphs":candidates.iter().map(|c| json!({"id":c.id,"gain_db":c.gain,"peak":c.peak,"pcm_peak":c.pcm_peak,"clipped_samples":c.clips,
                "path":c.path,"sha256":c.sha,"score":(c.peak-target).abs()})).collect::<Vec<_>>()}));
    }
    let statuses: Vec<_> = reply.events.iter().filter_map(|e| e.tool.as_ref().map(|tool|json!({"tool":tool,"success":e.success}))).collect();
    (json!({"state":reply.state,"model_calls":reply.model_calls,"tool_calls":reply.tool_calls,
        "target_peak":target,"tool_statuses":statuses,"workflow_evidence":checks,"accepted":good}),good,accepted_gains,chosen_sha)
}

#[cfg(windows)]
#[test]
#[ignore = "paid real API two-turn selection; explicit approval and settings path required"]
fn live_workflow_selection_feedback_two_turns() {
    assert_eq!(std::env::var("AUDIOPROCESS_LIVE_APPROVED").as_deref(),Ok("yes"));
    let settings_path = PathBuf::from(std::env::var_os("AUDIOPROCESS_LIVE_SETTINGS_PATH").expect("settings path required"));
    let config = crate::ai_settings::SettingsStore::default().load(&settings_path)
        .unwrap_or_else(|_|panic!("cannot load approved live settings")).expect("live settings missing");
    tauri::async_runtime::block_on(async {
        super::end_to_end::ensure_test_sidecar();
        let (root,spaces,backend) = test_backend();
        std::fs::create_dir_all(&spaces.ai_root).unwrap();
        let input = synthetic_input();
        std::fs::write(spaces.user_root.join("input.wav"),&input).unwrap();
        let original_sha = hash(&spaces.user_root.join("input.wav")).unwrap();
        let report_dir = std::env::var_os("AUDIOPROCESS_LIVE_REPORT_DIR").map(PathBuf::from).unwrap_or_else(||
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../out").join(format!("live-selection-{}-{}",
                std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())));
        std::fs::create_dir_all(report_dir.parent().unwrap()).unwrap();
        std::fs::create_dir(&report_dir).expect("report directory must be fresh");
        let data = root.join("data");
        let conversations = Arc::new(ConversationStore::default());
        let conversation = conversations.create(&spaces,&data,"workflow",Some("Synthetic selection feedback")).unwrap();
        let runs = Arc::new(RunStore::default());
        let prompts = [
            "这是隔离合成音频任务。用户空间 input.wav 是48kHz单声道PCM16、峰值0.5。自行发现节点能力与Workflow格式，编写JSON Workflow文件并校验，用for_each比较gain [-12,0,12]。候选文件名用concat生成，图必须测量abs peak和clipped_samples，用compute计算峰值与目标0.4差值的绝对值，filter排除clipped_samples>0，再select选最小分数，Workflow输出selected/gain/path/score。只能通过workflow_run执行图。所有中间文件放AI空间，保留原始input.wav，这轮先不要导出。不要写脚本或HTTP，不要声称听过音频。最多16次模型回复/40次工具调用。",
            "根据反馈目标峰值改为0.2。先读取上轮实际运行记录，使用至少3个都不同于上轮的新gain值细化候选。创建并校验新或参数化JSON Workflow并运行，使用新输出路径防止覆盖上轮结果；继续for_each、concat、compute绝对峰值差值、filter无clipping、select最小分数，只能workflow_run执行图。只将本轮最佳文件通过file_export交付用户 delivery/result.wav，检查交付audio_inspect。保持input.wav原字节，所有中间文件在AI空间，不要声称听过音频。最多16次模型回复/40次工具调用。"
        ];
        let mut summaries = Vec::new(); let mut accepted = Vec::new(); let mut prior_gains = Vec::new();
        let mut best_sha = None; let mut export_count = 0; let mut first_exports = 0;
        let mut history = false; let mut inspected = false; let mut discoveries = false;
        let mut completed = true; let mut first_record_ids = Vec::new();
        let mut workflow_only = true;
        for (index,prompt) in prompts.iter().enumerate() {
            // A fresh manager and store ensure turn two continues persisted conversation history.
            let manager = AgentManager::default();
            let conversation_store = Arc::new(ConversationStore::default());
            let request = format!("live-selection-{index}");
            let (cancel,lease) = manager.begin_conversation(&request,Some(conversation.id.clone())).unwrap();
            let guard = conversation_store.begin_turn(&spaces,&data,&conversation.id,"workflow",&request,prompt,None).unwrap();
            println!("LIVE selection turn={} starting, limits={} models/{} tools, 600s deadline",index+1,MAX_MODEL_CALLS,MAX_TOOL_CALLS);
            let deadline = cancel.clone();
            let timer = tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_secs(600)).await; deadline.store(true,Ordering::Release);
            });
            let reply = run_turn(&manager,Arc::new(AiManager::default()),backend.clone(),spaces.clone(),
                "script-user".into(),"workflow".into(),request,(*prompt).into(),
                AiConfig { base_url:config.base_url.clone(),model:config.model.clone(),api_key:config.api_key.clone() },None,cancel,
                Some((runs.clone(),data.clone())),Some(guard),Some(conversation.id.clone())).await;
            timer.abort(); drop(lease);
            let records: Vec<_> = runs.list(&spaces,&data).unwrap().records.into_iter()
                .map(|r|runs.load(&spaces,&data,&r.id).unwrap()).collect();
            let (summary,okay,gains,sha) = assess(&reply,&records,&spaces,if index==0 {0.4}else{0.2},index==0,&prior_gains);
            if index==0 { prior_gains=gains; } else { best_sha=sha; }
            let exports = reply.events.iter().filter(|e|e.tool.as_deref()==Some("file_export") && e.success==Some(true)).count();
            // Include exports inside Workflow, whose calls are recorded in the authoritative trace.
            let internal_exports = records.iter().filter(|r|r.kind=="workflow" && reply.events.iter().any(|e|
                e.tool.as_deref()==Some("workflow_run") && e.result.as_ref().and_then(|v|v.pointer("/data/run_id")).and_then(Value::as_str)==Some(r.id.as_str())))
                .filter_map(|r|r.result.as_ref()?.get("trace")?.as_array()).flatten()
                .filter(|t|t["tool"]=="file_export" && t["state"]=="succeeded").count();
            export_count += exports+internal_exports;
            if index==0 { first_exports=exports+internal_exports;
                discoveries=reply.events.iter().any(|e|e.tool.as_deref()==Some("nodes_list") && e.success==Some(true));
                first_record_ids=records.iter().map(|r|r.id.clone()).collect();
            } else {
                let first_execution=reply.events.iter().position(|e| e.tool.as_deref()==Some("workflow_run")).unwrap_or(reply.events.len());
                history=reply.events.iter().take(first_execution).any(|e|e.tool.as_deref()==Some("runs_read") && e.success==Some(true)
                    && e.arguments.as_ref().is_some_and(|a| a["id"].as_str().is_some_and(|id| first_record_ids.iter().any(|old|old==id))
                        && a["section"]=="result"));
                inspected=reply.events.iter().any(|e|e.tool.as_deref()==Some("audio_inspect") && e.success==Some(true)
                    && e.result.as_ref().is_some_and(|v|contains(v,"delivery/result.wav")));
            }
            completed &= reply.state=="completed";
            workflow_only &= !reply.events.iter().any(|e| e.tool.as_deref()==Some("graph_run"));
            println!("LIVE selection turn={} state={} models={} tools={}",index+1,reply.state,reply.model_calls,reply.tool_calls);
            summaries.push(summary); accepted.push(okay);
            std::fs::write(report_dir.join("progress.json"),serde_json::to_vec_pretty(&json!({"turns":summaries})).unwrap()).unwrap();
            for record in records.iter().filter(|r|r.kind=="workflow") {
                if let Some(document)=record.configuration.get("workflow") {
                    std::fs::write(report_dir.join(format!("executed-workflow-{}.json",record.id)),serde_json::to_vec_pretty(document).unwrap()).unwrap();
                }
            }
            if !completed { break; }
        }
        let preserved = hash(&spaces.user_root.join("input.wav")).as_deref()==Some(original_sha.as_str());
        let delivery_sha = hash(&spaces.user_root.join("delivery/result.wav"));
        let delivered = best_sha.is_some() && best_sha==delivery_sha;
        let persisted_turns = Arc::new(ConversationStore::default()).load(&spaces,&data,&conversation.id,None,None)
            .map(|c|c.turns.len()).unwrap_or(0);
        let mut files=Vec::new();
        let files_read=user_files(&spaces.user_root,&spaces.user_root,&mut files).is_ok();
        files.sort();
        let user_space_valid=files_read && files==vec!["delivery/result.wav".to_owned(),"input.wav".to_owned()];
        let shutdown = backend.shutdown().is_ok();
        let passed = completed && accepted.len()==2 && accepted.iter().all(|v|*v) && preserved && delivered
            && first_exports==0 && export_count==1 && history && inspected && discoveries && persisted_turns==2 && shutdown && user_space_valid && workflow_only;
        let summary=json!({"passed":passed,"turns":summaries,"original_sha256":original_sha,"input_preserved":preserved,
            "delivery_sha256":delivery_sha,"delivery_matches_selected":delivered,"first_turn_exports":first_exports,
            "total_exports":export_count,"history_read":history,"delivery_inspected":inspected,"nodes_discovered":discoveries,
            "persisted_turns":persisted_turns,"user_space_valid":user_space_valid,"workflow_only_execution":workflow_only,"shutdown":shutdown,"production_model_limit":MAX_MODEL_CALLS,
            "production_tool_limit":MAX_TOOL_CALLS,"fixture_path":root});
        std::fs::write(report_dir.join("report.json"),serde_json::to_vec_pretty(&summary).unwrap()).unwrap();
        std::fs::write(report_dir.join("synthetic-input.wav"),&input).unwrap();
        if delivered { std::fs::copy(spaces.user_root.join("delivery/result.wav"),report_dir.join("result.wav")).unwrap(); }
        println!("LIVE sanitized selection report: {}",report_dir.display());
        assert!(passed,"selection acceptance lacks authoritative evidence; inspect sanitized report");
        // Retain the unique fixture for local diagnosis; no actual application data is touched.
    });
}

#[test]
fn live_selection_receipt_path_matching_handles_windows_relative_paths() {
    for path in ["delivery/result.wav", "delivery\\result.wav", "C:\\fixture\\delivery\\result.wav"] {
        assert!(contains(&json!({"path":path}),"delivery/result.wav"));
    }
    assert!(!contains(&json!({"path":"wrongdelivery/result.wav"}),"delivery/result.wav"));
}

#[test]
fn live_selection_accepts_verified_projection_but_not_unrelated_outputs() {
    let c=Candidate{id:"graph-id".into(),gain:-4.45,peak:0.29955,clips:0.0,pcm_peak:0.29956,
        path:"r3/gain.wav".into(),sha:"hash".into(),iteration:"/loop/0/".into()};
    let selected=json!({"found":true,"item":{"run_id":"graph-id","path":"r3/gain.wav"},"index":3,"score":0.00045});
    let projection=json!({"gain":-4.45,"path":"r3\\gain.wav","score":0.00045});
    assert!(selection_outputs_match(&projection,&selected,&c));
    for (key,value) in [("gain",json!(-4.0)),("path",json!("another.wav")),("score",json!(0.1))] {
        let mut wrong=projection.clone();wrong[key]=value;
        assert!(!selection_outputs_match(&wrong,&selected,&c));
    }
    assert!(!selection_outputs_match(&json!({"all_candidates":[selected["item"]]}),&selected,&c));
}

#[test]
#[ignore = "paid fresh-conversation history recovery; explicit approval, settings and synthetic fixture required"]
fn live_fresh_conversation_recovers_selection_from_history() {
    use crate::backend::{Connection,DisconnectCallback};
    assert_eq!(std::env::var("AUDIOPROCESS_LIVE_APPROVED").as_deref(),Ok("yes"));
    let root = std::fs::canonicalize(PathBuf::from(std::env::var_os("AUDIOPROCESS_LIVE_FIXTURE_ROOT")
        .expect("existing synthetic fixture required"))).expect("fixture unavailable");
    assert_eq!(root.parent(),Some(std::fs::canonicalize(std::env::temp_dir()).unwrap().as_path()));
    assert!(root.file_name().unwrap().to_string_lossy().starts_with("audioprocess-agent-loop-"));
    assert_eq!(std::fs::read(root.join("user/input.wav")).unwrap(),synthetic_input(),"not the approved synthetic fixture");
    let settings = PathBuf::from(std::env::var_os("AUDIOPROCESS_LIVE_SETTINGS_PATH").expect("settings path required"));
    let config = crate::ai_settings::SettingsStore::default().load(&settings)
        .unwrap_or_else(|_|panic!("cannot load approved live settings")).expect("live settings missing");
    tauri::async_runtime::block_on(async {
        super::end_to_end::ensure_test_sidecar();
        let data = root.join("data");
        let spaces = ToolWorkspaces::new(&root.join("user"),&data).unwrap();
        let store = Arc::new(RunStore::default());
        let before: Vec<_> = store.list(&spaces,&data).unwrap().records.into_iter()
            .map(|r|store.load(&spaces,&data,&r.id).unwrap()).collect();
        assert!(before.iter().filter(|r|r.kind=="workflow" && r.state=="succeeded").count()>=2);
        let old_ids: HashSet<_> = before.iter().map(|r|r.id.clone()).collect();
        let previous_gains: Vec<_> = before.iter().filter_map(|r|candidate(r,&spaces)).map(|c|c.gain).collect();
        assert!(previous_gains.len()>=7,"previous candidates unavailable");
        let mut old_paths = Vec::new(); user_files(&spaces.user_root,&spaces.user_root,&mut old_paths).unwrap();
        let original_files: Vec<_> = old_paths.iter().map(|p|(p.clone(),hash(&spaces.user_root.join(p)).unwrap())).collect();
        let mut old_ai_paths = Vec::new(); user_files(&spaces.ai_root,&spaces.ai_root,&mut old_ai_paths).unwrap();
        let original_ai_files: Vec<_> = old_ai_paths.iter().map(|p|(p.clone(),hash(&spaces.ai_root.join(p)).unwrap())).collect();
        let report_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../out")
            .join(format!("live-history-{}-{}",std::process::id(),std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir(&report_dir).expect("fresh report directory required");
        let executable = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build/Debug/control-cli.exe");
        let callback: DisconnectCallback = Arc::new(|_|{});
        let session = "history-fresh-user";
        let backend = Arc::new(BackendManager::with_test_connection(Connection::spawn(
            &executable,spaces.user_root.clone(),session.into(),false,false,callback).unwrap()));
        let conversations = Arc::new(ConversationStore::default());
        let conversation = conversations.create(&spaces,&data,"workflow",Some("Fresh history-only continuation")).unwrap();
        assert!(conversation.turns.is_empty());
        let manager = AgentManager::default();
        let request = "live-fresh-history";
        let prompt = "这个工作区之前做过音频增益参数对比，但这是一个新会话。现在目标绝对峰值改为0.3。请自行查找历史运行记录，读取相关配置和实际结果明细，确认已有文件，再根据历史结果安排至少3个不同于历史的新候选增益继续细化。自己编写或调整并校验Workflow，用它运行原始输入的音频处理、计算与新目标的差值、排除削波候选并选出最小误差者，只交付本轮选中的音频到用户空间一个新子目录。所有中间文件放AI空间，不能覆盖已有文件，不要声称听过。只能通过Workflow运行音频，不要逐个直接graph_run。请尽量合并独立工具调用，在16次模型回复、40次工具调用内完成。";
        let guard = conversations.begin_turn(&spaces,&data,&conversation.id,"workflow",request,prompt,None).unwrap();
        assert!(guard.prior_turns.is_empty() && persisted_history(&guard).is_empty(),"old chat leaked into the new conversation");
        let (cancel,lease) = manager.begin_conversation(request,Some(conversation.id.clone())).unwrap();
        let deadline = cancel.clone();
        let timer = tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(600)).await; deadline.store(true,Ordering::Release);
        });
        println!("LIVE fresh history: empty conversation, target=0.3, no old ids/config/results in prompt; 600s deadline");
        let reply = run_turn(&manager,Arc::new(AiManager::default()),backend.clone(),spaces.clone(),
            session.into(),"workflow".into(),request.into(),prompt.into(),config,None,cancel,
            Some((store.clone(),data.clone())),Some(guard),Some(conversation.id.clone())).await;
        timer.abort(); drop(lease);
        let records: Vec<_> = store.list(&spaces,&data).unwrap().records.into_iter()
            .map(|r|store.load(&spaces,&data,&r.id).unwrap()).collect();
        let (assessment,selection_ok,_,selected_sha) = assess(&reply,&records,&spaces,0.3,false,&previous_gains);
        let first_run = reply.events.iter().position(|e|e.tool.as_deref()==Some("workflow_run")).unwrap_or(reply.events.len());
        let reads: Vec<_> = reply.events.iter().take(first_run).filter(|e| e.success==Some(true)
            && e.tool.as_deref()==Some("runs_read") && e.arguments.as_ref().is_some_and(|a|
                a["id"].as_str().is_some_and(|id|old_ids.contains(id)))).collect();
        let read_families = |section: &str| -> HashSet<String> {
            reads.iter().filter(|e|e.arguments.as_ref().is_some_and(|a|a["section"]==section)
                && e.result.as_ref().is_some_and(|r|r.pointer("/data/complete")==Some(&json!(true))
                    && r.pointer("/data/data").is_some_and(|value| meaningful_history_content(value,section))))
                .filter_map(|e| e.arguments.as_ref()?.get("id")?.as_str())
                .filter_map(|id| before.iter().find(|r|r.id==id))
                .map(|r|r.parent_id.clone().unwrap_or_else(||r.id.clone())).collect()
        };
        let mut config_families = read_families("configuration");
        // Reading the original workflow file is also valid configuration evidence.
        for event in reply.events.iter().take(first_run).filter(|e|e.success==Some(true) && e.tool.as_deref()==Some("file_read_text")) {
            if let Some(content)=event.result.as_ref().and_then(|r|r.pointer("/data/content")).and_then(Value::as_str) {
                let digest=format!("{:x}",Sha256::digest(content.as_bytes()));
                for record in before.iter().filter(|r|r.kind=="workflow" && r.configuration.pointer("/source/sha256").and_then(Value::as_str)==Some(digest.as_str())) {
                    config_families.insert(record.id.clone());
                }
            }
        }
        let result_families = read_families("result");
        let read_configuration = !config_families.is_empty();
        let read_result = !result_families.is_empty();
        let related_history = config_families.iter().any(|id|result_families.contains(id));
        let history_discovered = reply.events.iter().take(first_run).any(|e|e.tool.as_deref()==Some("runs_list") && e.success==Some(true));
        let originals_preserved = original_files.iter().all(|(p,sha)|hash(&spaces.user_root.join(p)).as_deref()==Some(sha));
        let ai_files_preserved = original_ai_files.iter().all(|(p,sha)|hash(&spaces.ai_root.join(p)).as_deref()==Some(sha));
        let checked_files = reply.events.iter().take(first_run).any(|e| e.success==Some(true)
            && matches!(e.tool.as_deref(),Some("workspace_list"|"audio_inspect"|"runs_check_files")));
        let records_preserved = before.iter().all(|old| records.iter().find(|r|r.id==old.id)
            .is_some_and(|current|serde_json::to_value(current).ok()==serde_json::to_value(old).ok()));
        let mut after_paths = Vec::new(); user_files(&spaces.user_root,&spaces.user_root,&mut after_paths).unwrap();
        let added: Vec<_> = after_paths.into_iter().filter(|p|!old_paths.contains(p)).collect();
        let delivery = added.len()==1 && added[0].split('/').count()==2
            && selected_sha.is_some() && hash(&spaces.user_root.join(&added[0]))==selected_sha;
        let export_confirmed = reply.events.iter().any(|e|e.tool.as_deref()==Some("file_export") && e.success==Some(true))
            || records.iter().filter(|r|r.kind=="workflow" && !old_ids.contains(&r.id))
                .filter_map(|r|r.result.as_ref()?.get("trace")?.as_array()).flatten()
                .any(|entry|entry["tool"]=="file_export" && entry["state"]=="succeeded");
        let workflow_only = !reply.events.iter().any(|e|e.tool.as_deref()==Some("graph_run"));
        let stored = conversations.load(&spaces,&data,&conversation.id,None,None).unwrap();
        let fresh_history = stored.turns.len()==1;
        let shutdown = backend.shutdown().is_ok();
        let passed = reply.state=="completed" && selection_ok && history_discovered && read_configuration
            && read_result && related_history && checked_files && originals_preserved && ai_files_preserved
            && records_preserved && delivery && export_confirmed && workflow_only && fresh_history && shutdown;
        let summary = json!({"passed":passed,"initial_history_turns":0,"final_history_turns":stored.turns.len(),
            "prompt":prompt,"assessment":assessment,"history_discovered":history_discovered,
            "old_configuration_read":read_configuration,"old_result_detail_read":read_result,
            "related_configuration_and_result":related_history,"existing_files_checked":checked_files,
            "old_ai_files_preserved":ai_files_preserved,"file_export_confirmed":export_confirmed,
            "history_reads_before_execution":reads.iter().map(|e|json!({"id":e.arguments.as_ref().unwrap()["id"],
                "section":e.arguments.as_ref().unwrap()["section"],"pointer":e.arguments.as_ref().unwrap().get("pointer"),
                "complete":e.result.as_ref().and_then(|r|r.pointer("/data/complete"))})).collect::<Vec<_>>(),
            "previous_gains_for_verifier_only":previous_gains,"existing_user_files_preserved":originals_preserved,
            "old_records_unchanged":records_preserved,"added_user_files":added,"delivery_matches_selected":delivery,
            "delivery_sha256":selected_sha,"workflow_only_execution":workflow_only,"shutdown":shutdown,"fixture_path":root});
        std::fs::write(report_dir.join("report.json"),serde_json::to_vec_pretty(&summary).unwrap()).unwrap();
        for record in records.iter().filter(|r|r.kind=="workflow" && !old_ids.contains(&r.id)) {
            if let Some(document)=record.configuration.get("workflow") {
                std::fs::write(report_dir.join(format!("executed-workflow-{}.json",record.id)),serde_json::to_vec_pretty(document).unwrap()).unwrap();
            }
        }
        if delivery { std::fs::copy(spaces.user_root.join(&added[0]),report_dir.join("result.wav")).unwrap(); }
        println!("LIVE fresh history state={} models={} tools={} passed={}",reply.state,reply.model_calls,reply.tool_calls,passed);
        println!("LIVE sanitized fresh-history report: {}",report_dir.display());
        assert!(passed,"fresh history continuation lacks required evidence; inspect sanitized report");
    });
}

fn meaningful_history_content(value: &Value, section: &str) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key,value)| {
            let names: &[&str] = if section=="configuration" { &["nodes","steps","gain_db","gains"] }
                else { &["peak","clipped_samples","score","distance","selected"] };
            (names.contains(&key.as_str()) && !value.is_null()) || meaningful_history_content(value,section)
        }),
        Value::Array(array) => array.iter().any(|v|meaningful_history_content(v,section)),
        _ => false,
    }
}
