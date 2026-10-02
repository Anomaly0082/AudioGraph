//! Opt-in provider smoke. Ordinary tests never read credentials or use paid APIs.
use super::*;
use std::io::Read;

#[cfg(windows)]
#[test]
#[ignore = "paid model smoke: explicit approval and AiConfig JSON on closed stdin required"]
fn live_workflow_feedback_smoke() {
    assert_eq!(std::env::var("AUDIOPROCESS_LIVE_APPROVED").as_deref(), Ok("yes"));
    let mut input = String::new();
    std::io::stdin().lock().take(16 * 1024 + 1).read_to_string(&mut input)
        .expect("cannot read smoke config from stdin");
    assert!(input.len() <= 16 * 1024, "smoke config exceeds limit");
    // Never print config/credentials or pass them to the audio child via argv/env.
    let config: AiConfig = serde_json::from_str(&input).unwrap_or_else(|_| panic!("invalid smoke config"));
    drop(input);
    tauri::async_runtime::block_on(async {
        super::end_to_end::ensure_test_sidecar();
        let (root, spaces, backend) = test_backend();
        let mut wav = Vec::new();
        let samples = 4800_u32;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + samples * 2).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&48000_u32.to_le_bytes());
        wav.extend_from_slice(&96000_u32.to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(samples * 2).to_le_bytes());
        for _ in 0..samples { wav.extend_from_slice(&8192_i16.to_le_bytes()); }
        std::fs::write(spaces.user_root.join("input.wav"), &wav).unwrap();
        let manager = AgentManager::default();
        let store = Arc::new(RunStore::default());
        let (cancel, lease) = manager.begin("live-workflow-smoke").unwrap();
        let deadline_cancel = cancel.clone();
        let timer = tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(120)).await;
            deadline_cancel.store(true, Ordering::Release);
        });
        let prompt = "这是临时测试工作区，只有合成音频。请完成一次简短闭环：先查看工作区和节点能力，把 input.wav 复制到AI空间。创建并校验一个参数化 Workflow JSON，内部用Graph读取该音频、gain处理、peak_meter测峰值并写新WAV，返回峰值和输出路径。先运行 gain_db=-6，读取实际峰值；如果小于0.2，再用 gain_db=0 和不同输出路径运行同一个Workflow。只将第二次音频导出到用户空间 smoke-delivery/result.wav，不交付探针。查看一次本次运行历史并检查交付文件的元数据。尽量合并独立工具请求，控制在16次模型回复内；最终只简短说明成功或失败和实际峰值，不声称听过。不要调用HTTP或编写脚本。";
        println!("LIVE smoke: synthetic WAV only, <=16 model requests/40 tools, 120s cooperative deadline");
        let response = run_turn(&manager, Arc::new(AiManager::default()), backend.clone(), spaces.clone(),
            "script-user".into(), "workflow".into(), "live-workflow-smoke".into(), prompt.into(),
            config, None, cancel, Some((store.clone(), root.join("data"))), None, None).await;
        timer.abort();
        drop(lease);
        println!("LIVE state={} model_calls={} tool_calls={} run_ids={}", response.state,
            response.model_calls, response.tool_calls, response.run_ids.len());
        for event in &response.events {
            if let Some(tool) = &event.tool {
                println!("LIVE tool={} success={}", tool, event.success.unwrap_or(false));
                if event.success == Some(false) {
                    // Tool errors come from the isolated synthetic workspace, not config or model reasoning.
                    let error = event.result.as_ref().and_then(|v| v.get("error")).cloned().unwrap_or(Value::Null);
                    println!("LIVE tool error={}", bounded_text(&error.to_string(), 600));
                }
            }
        }
        let successful = |name: &str| response.events.iter()
            .filter(|e| e.tool.as_deref() == Some(name) && e.success == Some(true)).count();
        let delivered = std::fs::read(spaces.user_root.join("smoke-delivery/result.wav")).ok();
        let input_unchanged = std::fs::read(spaces.user_root.join("input.wav")).unwrap() == wav;
        // Inspect executed Graph snapshots, not the model's claims. Keep round order
        // from tool receipts rather than timestamps that can tie for tiny inputs.
        let mut variants = Vec::new();
        let mut sources = Vec::new();
        for event in response.events.iter().filter(|e| e.tool.as_deref() == Some("workflow_run") && e.success == Some(true)) {
            let report = event.result.as_ref().unwrap();
            sources.push(report.pointer("/data/source/sha256").cloned().unwrap_or(Value::Null));
            let mut ids = Vec::new();
            collect_run_ids(report, &mut ids);
            for id in ids {
                let record = store.load(&spaces,&root.join("data"),&id).unwrap();
                if record.kind != "graph" { continue; }
                let graph = &record.configuration["graph"];
                let nodes = graph["nodes"].as_array().unwrap();
                let gain = nodes.iter().find(|n| n["type"] == "gain").unwrap()["parameters"]["gain_db"].as_f64().unwrap();
                let meter = nodes.iter().find(|n| n["type"] == "peak_meter").unwrap()["id"].as_str().unwrap();
                let export = graph["exports"].as_array().unwrap().iter().find(|e| e["node"] == meter && e["port"] == "peak").unwrap()["name"].as_str().unwrap();
                let result = record.result.as_ref().unwrap();
                let peak = result["result"]["outputs"][export]["value"].as_f64().unwrap();
                let output = nodes.iter().find(|n| n["type"] == "wav_output").unwrap()["parameters"]["path"].as_str().unwrap().to_owned();
                variants.push((gain,peak,output));
            }
        }
        backend.shutdown().unwrap();
        // Only remove the unique fixture returned by test_backend, never an app/user workspace.
        assert!(root.file_name().unwrap().to_string_lossy().starts_with("audioprocess-agent-loop-"));
        assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(response.state, "completed", "live turn did not complete; see sanitized tool statuses");
        for name in ["workspace_list", "nodes_list", "workflow_validate", "file_export", "audio_inspect"] {
            assert!(successful(name) > 0, "missing successful {name}");
        }
        assert!(successful("runs_read") + successful("runs_list") > 0, "missing history lookup");
        assert!(successful("workflow_run") >= 2, "feedback did not run two workflow variants");
        assert_eq!(variants.len(),2,"expected two real Graph variants");
        assert!((variants[0].0+6.0).abs()<0.001 && variants[0].1<0.2,"baseline did not use -6 dB and measure the threshold");
        assert!(variants[1].0.abs()<0.001 && (variants[1].1-0.25).abs()<0.001,"second run did not apply the required feedback adjustment");
        assert_ne!(variants[0].2,variants[1].2,"variant paths must differ");
        assert!(!sources[0].is_null() && sources[0]==sources[1],"must rerun the same parameterized workflow snapshot");
        assert!(response.run_ids.len() >= 4, "missing workflow/graph run links");
        assert!(input_unchanged, "synthetic input changed");
        let delivered = delivered.expect("no delivered WAV");
        assert_eq!(delivered.len(), wav.len());
        assert!(delivered[44..].chunks_exact(2).all(|v| i16::from_le_bytes([v[0],v[1]]) == 8192),
            "delivered samples do not match the feedback-selected 0 dB variant");
        println!("LIVE PASS: actual model tools, peak {:.6} -> {:.6}, two Workflow/C++ variants, history and folder export; source preserved",variants[0].1,variants[1].1);
    });
}
