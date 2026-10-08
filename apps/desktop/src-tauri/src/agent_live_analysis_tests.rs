//! Opt-in paid acceptance; reports contain execution identities and numbers only.
use super::*;
use crate::run_records::RunRecord;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const ANALYSIS: &str = "org.audiograph.analysis.audio_metrics_v1";
fn hash(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|b| format!("{:x}", Sha256::digest(b)))
}
fn sample(i: usize) -> i16 {
    let level = if i < 960 { 0 } else if i < 1920 { 26 } else { 16384 };
    if i % 2 == 0 { level } else { -level }
}
fn fixture() -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF"); b.extend_from_slice(&9636_u32.to_le_bytes());
    b.extend_from_slice(b"WAVEfmt "); b.extend_from_slice(&16_u32.to_le_bytes());
    b.extend_from_slice(&1_u16.to_le_bytes()); b.extend_from_slice(&1_u16.to_le_bytes());
    b.extend_from_slice(&48000_u32.to_le_bytes()); b.extend_from_slice(&96000_u32.to_le_bytes());
    b.extend_from_slice(&2_u16.to_le_bytes()); b.extend_from_slice(&16_u16.to_le_bytes());
    b.extend_from_slice(b"data"); b.extend_from_slice(&9600_u32.to_le_bytes());
    for i in 0..4800 { b.extend_from_slice(&sample(i).to_le_bytes()); }
    b
}
fn expected(gain: f64) -> (f64,f64,f64) {
    let multiplier = 10.0_f64.powf(gain/20.0) as f32;
    let values: Vec<f64> = (0..4800).map(|i| ((f32::from(sample(i))/32768.0)*multiplier) as f64).collect();
    let rms = (values.iter().map(|x|x*x).sum::<f64>()/4800.0).sqrt();
    let clips = values.iter().filter(|x| **x < -1.0 || **x > 32767.0/32768.0).count() as f64/4800.0;
    let quiet = values.chunks(960).filter(|w| (w.iter().map(|x|x*x).sum::<f64>()/w.len() as f64).sqrt() <= 0.001)
        .map(|w|w.len()).sum::<usize>() as f64/4800.0;
    (rms,clips,quiet)
}
fn pcm_rms(path: &Path) -> Option<f64> {
    let b=std::fs::read(path).ok()?;
    if b.get(..4)? != b"RIFF" || b.get(8..12)? != b"WAVE" { return None; }
    let mut cursor=12; let mut format=false; let mut rms=None;
    while cursor+8<=b.len() {
        let length=u32::from_le_bytes(b[cursor+4..cursor+8].try_into().ok()?) as usize;
        let start=cursor+8; let end=start.checked_add(length)?; let data=b.get(start..end)?;
        if &b[cursor..cursor+4]==b"fmt " && data.len()>=16 {
            format=data[..4]==[1,0,1,0] && data[4..8]==48000_u32.to_le_bytes()
                && data[12..16]==[2,0,16,0];
        }
        if &b[cursor..cursor+4]==b"data" {
            if length!=9600 { return None; }
            rms=Some((data.chunks_exact(2).map(|s| {
                let v=f64::from(i16::from_le_bytes([s[0],s[1]]))/32768.0; v*v
            }).sum::<f64>()/4800.0).sqrt());
        }
        cursor=end.checked_add(length%2)?;
    }
    if format { rms } else { None }
}
fn contains(v: &Value, text: &str) -> bool {
    match v {
        Value::String(s) => { let s=s.replace('\\',"/"); s==text || s.ends_with(&format!("/{text}")) },
        Value::Object(o) => o.values().any(|v|contains(v,text)),
        Value::Array(a) => a.iter().any(|v|contains(v,text)), _ => false,
    }
}
struct Candidate { id:String,gain:f64,rms:f64,clips:f64,quiet:f64,path:String,sha:String,iteration:String }
fn candidate(r: &RunRecord, spaces: &ToolWorkspaces) -> Option<Candidate> {
    if r.kind!="graph" || r.state!="succeeded" { return None; }
    let graph=&r.configuration["graph"]; let nodes=graph["nodes"].as_array()?;
    let gain_nodes:Vec<_>=nodes.iter().filter(|n|n["type"]=="gain").collect();
    if gain_nodes.len()!=1 { return None; }
    let gain=gain_nodes[0]["parameters"]["gain_db"].as_f64()?;
    let analysis=nodes.iter().find(|n|n["type"]==ANALYSIS)?;
    let threshold=analysis["parameters"].get("silence_threshold_db").map(Value::as_f64).unwrap_or(Some(-60.0));
    if threshold!=Some(-60.0) { return None; }
    let outputs=r.result.as_ref()?.pointer("/result/outputs")?;
    let metric=|port:&str| -> Option<f64> {
        let export=graph["exports"].as_array()?.iter().find(|e|e["node"]==analysis["id"] && e["port"]==port)?;
        outputs.get(export["name"].as_str()?)?.get("value")?.as_f64()
    };
    let (rms,clips,quiet)=(metric("rms")?,metric("clipping_ratio")?,metric("silence_ratio")?);
    let theory=expected(gain);
    if ![rms,clips,quiet].iter().all(|x|x.is_finite()) || (rms-theory.0).abs()>1e-6
        || (clips-theory.1).abs()>1e-9 || (quiet-theory.2).abs()>1e-9 { return None; }
    if let Some(db)=metric("rms_dbfs") { if (db-20.0*rms.log10()).abs()>1e-5 { return None; } }
    let file=r.files.iter().find(|f|f.space=="ai" && f.role=="output" && f.sha256.is_some())?;
    let sha=file.sha256.clone()?;
    if hash(&spaces.ai_root.join(&file.path)).as_deref()!=Some(&sha) { return None; }
    let step=r.configuration["workflow_step_path"].as_str()?;
    if !step.contains("/iterations/") { return None; }
    Some(Candidate{id:r.id.clone(),gain,rms,clips,quiet,path:file.path.clone(),sha,
        iteration:format!("{}/",step.rsplit_once('/')?.0)})
}
fn matches(v:&Value,c:&Candidate)->bool { contains(v,&c.id) && contains(v,&c.path) }
fn user_files(dir:&Path,base:&Path,files:&mut Vec<String>) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir)? { let e=e?;
        if e.file_type()?.is_dir() { user_files(&e.path(),base,files)?; }
        else { files.push(e.path().strip_prefix(base).unwrap().to_string_lossy().replace('\\',"/")); }
    } Ok(())
}

#[test]
fn synthetic_analysis_fixture_has_independent_rejection_cases() {
    assert_eq!(fixture().len(),44+4800*2);
    let expected_cases=[(-6.0,0.1941090514222284,0.0,0.4),
        (0.0,0.387298497176031,0.0,0.4),(3.0,0.5470736799123238,0.0,0.2),
        (12.0,1.5418630906601989,0.6,0.2)];
    for (gain,rms,clips,quiet) in expected_cases {
        let actual=expected(gain);
        assert!((actual.0-rms).abs()<1e-9);
        assert_eq!((actual.1,actual.2),(clips,quiet));
        assert_eq!(actual.1==0.0 && actual.2<=0.3,gain==3.0);
    }
}

#[test]
#[ignore = "paid analysis plugin acceptance; approval and settings path required"]
fn live_analysis_plugin_workflow_selection() {
    assert_eq!(std::env::var("AUDIOPROCESS_LIVE_APPROVED").as_deref(),Ok("yes"));
    let settings=PathBuf::from(std::env::var_os("AUDIOPROCESS_LIVE_SETTINGS_PATH").expect("settings path required"));
    let config=crate::ai_settings::SettingsStore::default().load(&settings)
        .unwrap_or_else(|_|panic!("cannot load approved settings")).expect("settings missing");
    tauri::async_runtime::block_on(async {
        super::end_to_end::ensure_test_sidecar();
        let suffix=format!("{}-{}",std::process::id(),std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        let root=std::env::temp_dir().join(format!("audioprocess-analysis-live-{suffix}"));
        let user=root.join("user"); let data=root.join("data"); let plugins=root.join("configplugins");
        for dir in [&user,&data,&plugins] { std::fs::create_dir_all(dir).unwrap(); }
        let spaces=ToolWorkspaces::new(&user,&data).unwrap();
        let repo=std::fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")).unwrap();
        let installer=crate::plugin_manager::PluginManager::default();
        installer.initialize(&plugins,&data).unwrap();
        installer.install(&repo.join("build/audio-analysis-sdk-standalone/plugin-build/package/Release"),true).unwrap();
        let snapshot=crate::plugin_manager::PluginManager::default().initialize(&plugins,&data).unwrap();
        let backend=Arc::new(BackendManager::default()); backend.configure_plugin_snapshot(Ok(snapshot)).unwrap();
        let session=backend.connect(user.to_string_lossy().into_owned(),false,false,Arc::new(|_|{})).unwrap().session_id;
        std::fs::write(user.join("input.wav"),fixture()).unwrap();
        let input_sha=hash(&user.join("input.wav")).unwrap();
        let report_dir=repo.join("out").join(format!("live-analysis-{suffix}"));
        std::fs::create_dir_all(report_dir.parent().unwrap()).unwrap();
        std::fs::create_dir(&report_dir).expect("report directory must be fresh");
        let conversations=Arc::new(ConversationStore::default());
        let conversation=conversations.create(&spaces,&data,"workflow",Some("Isolated analysis acceptance")).unwrap();
        let request="live-analysis-selection";
        let prompt="请查可用能力，比较input.wav的-6、0、3、12dB增益，要求整体RMS尽量接近0.55（线性），不能PCM16削波，低电平判定阈值-60dBFS，其时长占比不超过30%；编写并校验Workflow执行候选、过滤并选优，仅把本轮最佳导出到用户空间一个新子目录。所有中间文件在AI空间，保留原输入。禁止脚本和直接graph_run；不要声称听过。限制16次模型回复、40次工具调用。";
        let guard=conversations.begin_turn(&spaces,&data,&conversation.id,"workflow",request,prompt,None).unwrap();
        assert!(!prompt.contains(ANALYSIS) && !prompt.contains("Audio Metrics"));
        assert!(guard.prior_turns.is_empty() && persisted_history(&guard).is_empty());
        let manager=AgentManager::default(); let runs=Arc::new(RunStore::default());
        let (cancel,lease)=manager.begin_conversation(request,Some(conversation.id.clone())).unwrap();
        let deadline=cancel.clone();
        let timer=tauri::async_runtime::spawn(async move { tokio::time::sleep(Duration::from_secs(600)).await;
            deadline.store(true,Ordering::Release); });
        println!("LIVE analysis starting models=16 tools=40 deadline_seconds=600");
        let reply=run_turn(&manager,Arc::new(AiManager::default()),backend.clone(),spaces.clone(),session,
            "workflow".into(),request.into(),prompt.into(),config,None,cancel,
            Some((runs.clone(),data.clone())),Some(guard),Some(conversation.id.clone())).await;
        timer.abort(); drop(lease);
        let records:Vec<_>=runs.list(&spaces,&data).unwrap().records.into_iter()
            .map(|r|runs.load(&spaces,&data,&r.id).unwrap()).collect();
        let discovered=reply.events.iter().any(|e|e.tool.as_deref()==Some("nodes_list") && e.success==Some(true)
            && e.result.as_ref().is_some_and(|v|contains(v,ANALYSIS)));
        let mut evidence=Vec::new(); let mut chosen_sha=None; let mut chosen_rms=None; let mut accepted=false;
        for w in records.iter().filter(|r|r.kind=="workflow" && r.state=="succeeded") {
            let graph_count=records.iter().filter(|r|r.kind=="graph" && r.parent_id.as_deref()==Some(w.id.as_str())).count();
            let cs:Vec<_>=records.iter().filter(|r|r.parent_id.as_deref()==Some(w.id.as_str()))
                .filter_map(|r|candidate(r,&spaces)).collect();
            let result=w.result.as_ref().unwrap_or(&Value::Null);
            let trace=result["trace"].as_array().cloned().unwrap_or_default();
            let filter=trace.iter().rev().find(|t|t["step_type"]=="filter" && t["state"]=="succeeded")
                .and_then(|t|t["result"].as_array());
            let select=trace.iter().rev().find(|t|t["step_type"]=="select" && t["state"]=="succeeded").map(|t|&t["result"]);
            let selected=select.and_then(|s|cs.iter().find(|c|matches(&s["item"],c)));
            let gains=graph_count==4 && cs.len()==4 && [-6.0,0.0,3.0,12.0].iter().all(|g|cs.iter().filter(|c|c.gain==*g).count()==1);
            let filtered=filter.is_some_and(|a|a.len()==1 && cs.iter().all(|c|
                a.iter().any(|v|matches(v,c))==(c.clips==0.0 && c.quiet<=0.3)));
            let selection=select.is_some_and(|s|s["found"]==true && s["index"].as_u64()
                .and_then(|i|filter.and_then(|a|a.get(i as usize)))==Some(&s["item"]))
                && selected.is_some_and(|c|c.gain==3.0 && select.and_then(|s|s["score"].as_f64())
                    .is_some_and(|score|(score-(c.rms-0.55).abs()).abs()<1e-6));
            let compute=cs.iter().all(|c|trace.iter().any(|t|t["step_type"]=="compute" && t["state"]=="succeeded"
                && t["step_path"].as_str().is_some_and(|p|p.starts_with(&c.iteration))
                && t["result"].as_f64().is_some_and(|s|(s-(c.rms-0.55).abs()).abs()<1e-6)));
            let sha=w.configuration.pointer("/source/sha256").and_then(Value::as_str);
            let validated=sha.is_some() && reply.events.iter().any(|e|e.tool.as_deref()==Some("workflow_validate")
                && e.success==Some(true) && e.result.as_ref().and_then(|v|v.pointer("/data/source/sha256"))
                    .and_then(Value::as_str)==sha);
            let executed=reply.events.iter().any(|e|e.tool.as_deref()==Some("workflow_run") && e.success==Some(true)
                && e.result.as_ref().and_then(|v|v.pointer("/data/run_id")).and_then(Value::as_str)==Some(w.id.as_str()));
            let okay=gains && filtered && selection && compute && validated && executed;
            if okay { accepted=true; chosen_sha=selected.map(|c|c.sha.clone()); chosen_rms=selected.map(|c|c.rms); }
            evidence.push(json!({"workflow_id":w.id,"source_sha256":sha,"executed":executed,"validated":validated,
                "graph_count":graph_count,"gains_valid":gains,"filter_valid":filtered,"selection_valid":selection,"compute_valid":compute,
                "selected":selected.map(|c|json!({"gain_db":c.gain,"path":c.path,"sha256":c.sha})),
                "graphs":cs.iter().map(|c|json!({"id":c.id,"gain_db":c.gain,"rms":c.rms,"clipping_ratio":c.clips,
                    "silence_ratio":c.quiet,"path":c.path,"sha256":c.sha})).collect::<Vec<_>>()}));
        }
        let mut files=Vec::new(); user_files(&user,&user,&mut files).unwrap();
        let added:Vec<_>=files.iter().filter(|p|p.as_str()!="input.wav").collect();
        let top_entries:Vec<_>=std::fs::read_dir(&user).unwrap().map(|e|e.unwrap()).collect();
        let directories=top_entries.iter().filter(|e|e.file_type().is_ok_and(|t|t.is_dir())).count();
        let delivery=top_entries.len()==2 && directories==1 && added.len()==1 && added[0].split('/').count()==2 && added[0].ends_with(".wav")
            && chosen_sha.is_some() && hash(&user.join(added[0]))==chosen_sha
            && pcm_rms(&user.join(added[0])).zip(chosen_rms).is_some_and(|(a,b)|(a-b).abs()<=1.0/32768.0);
        let exported=reply.events.iter().any(|e|e.tool.as_deref()==Some("file_export") && e.success==Some(true))
            || records.iter().filter(|r|r.kind=="workflow").filter_map(|r|r.result.as_ref()?.get("trace")?.as_array())
                .flatten().any(|t|t["tool"]=="file_export" && t["state"]=="succeeded");
        let preserved=hash(&user.join("input.wav")).as_deref()==Some(&input_sha);
        let workflow_only=!reply.events.iter().any(|e|e.tool.as_deref()==Some("graph_run"));
        let shutdown=backend.shutdown().is_ok();
        let passed=reply.state=="completed" && discovered && accepted && delivery && exported && preserved && workflow_only && shutdown;
        let report=json!({"passed":passed,"state":reply.state,"model_calls":reply.model_calls,"tool_calls":reply.tool_calls,
            "prompt":prompt,"initial_history_turns":0,"plugin_identity_supplied_in_prompt":false,
            "tool_statuses":reply.events.iter().filter_map(|e|e.tool.as_ref().map(|t|json!({"tool":t,"success":e.success}))).collect::<Vec<_>>(),
            "discovered":discovered,"workflows":evidence,"delivery_valid":delivery,"file_export_confirmed":exported,
            "added_user_files":added,"input_sha256":input_sha,"input_preserved":preserved,"workflow_only":workflow_only,
            "shutdown":shutdown,"fixture_path":root});
        std::fs::write(report_dir.join("report.json"),serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        if delivery { std::fs::copy(user.join(added[0]),report_dir.join("result.wav")).unwrap(); }
        for w in records.iter().filter(|r|r.kind=="workflow") { if let Some(document)=w.configuration.get("workflow") {
            std::fs::write(report_dir.join(format!("executed-workflow-{}.json",w.id)),serde_json::to_vec_pretty(document).unwrap()).unwrap();
        } }
        println!("LIVE analysis state={} models={} tools={} passed={}",reply.state,reply.model_calls,reply.tool_calls,passed);
        println!("LIVE analysis report: {}",report_dir.display());
        assert!(passed,"analysis acceptance failed; inspect sanitized report");
    });
}
