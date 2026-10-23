use super::*;
use std::sync::atomic::AtomicU64;

#[cfg(windows)]
#[path = "audio_conversion_asr_tests.rs"]
mod audio_conversion_asr_tests;

static NEXT: AtomicU64 = AtomicU64::new(0);

#[cfg(windows)]
#[test]
fn real_cpp_select_gain_workflow_exports_only_an_unclipped_winner() {
    use crate::backend::{Connection,DisconnectCallback};
    tauri::async_runtime::block_on(async {
        let executable_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let executable = [executable_root.join("build/Debug/control-cli.exe"),executable_root.join("build/Release/control-cli.exe")]
            .into_iter().find(|path| path.is_file()).expect("Build the C++ control-cli before running integration tests");
        // Fresh temporary workspaces keep both scenarios independent of devices and user files.
        for gains in [json!([-12,0,12]),json!([12])] {
            let (root,spaces) = spaces();
            let mut wav = vec![
                b'R',b'I',b'F',b'F',44,0,0,0,b'W',b'A',b'V',b'E',b'f',b'm',b't',b' ',
                16,0,0,0,1,0,1,0,0x44,0xac,0,0,0x88,0x58,1,0,2,0,16,0,
                b'd',b'a',b't',b'a',8,0,0,0,
            ];
            for sample in [16384_i16,-16384,4096,-4096] { wav.extend_from_slice(&sample.to_le_bytes()); }
            std::fs::write(spaces.user_root.join("input.wav"),&wav).unwrap();
            let mut program: Value = serde_json::from_str(include_str!("../../../../examples/workflows/select-gain.workflow.json")).unwrap();
            program["inputs"]["gains"] = gains.clone();
            std::fs::write(spaces.user_root.join("select.workflow.json"),program.to_string()).unwrap();
            let callback: DisconnectCallback = Arc::new(|_| {});
            let shared = Arc::new(BackendManager::with_test_connection(Connection::spawn(
                &executable,spaces.user_root.clone(),"select-user".into(),false,false,callback.clone()).unwrap()));
            let owned = Arc::new(BackendManager::with_test_connection(Connection::spawn(
                &executable,spaces.ai_root.clone(),"select-ai".into(),false,false,callback).unwrap()));
            let store = Arc::new(RunStore::default());
            let app_data = root.join("data");
            let mut context = ToolContext::new(spaces,shared.clone(),"select-user".into())
                .with_records(store.clone(),app_data.clone());
            context.owned = Some((owned.clone(),"select-ai".into()));
            let cancel = AtomicBool::new(false);
            let args = json!({"space":"user","path":"select.workflow.json"});
            let validated = context.dispatch("workflow","workflow_validate",&args,&cancel).await.unwrap();
            assert_eq!(validated["valid"],true,"{validated}");
            assert!(!context.spaces.ai_root.join("select-gain-001").exists());
            assert!(!context.spaces.user_root.join("select-gain-001").exists());
            assert!(store.list(&context.spaces,&app_data).unwrap().records.is_empty());
            let report = context.dispatch("workflow","workflow_run",&args,&cancel).await.unwrap();
            assert_eq!(report["state"],"succeeded","{report}");
            let count = gains.as_array().unwrap().len();
            assert_eq!(report["graph_runs"].as_u64(),Some(count as u64));
            let runs = report["outputs"]["all_results"].as_array().unwrap();
            assert_eq!(runs.len(),count);
            for (index,(run,gain)) in runs.iter().zip(gains.as_array().unwrap()).enumerate() {
                assert_eq!(&run["gain_db"],gain);
                assert_eq!(run["source_index"].as_u64(),Some(index as u64));
                let gain = gain.as_f64().unwrap();
                let path = format!("select-gain-001/gain-{}.wav",gain as i32);
                assert_eq!(run["output_path"],path);
                assert_eq!(run["render"]["state"],"succeeded");
                let outputs = &run["render"]["result"]["outputs"];
                let peak = outputs["peak"]["value"].as_f64().unwrap();
                let expected = 0.5*10.0_f64.powf(gain/20.0);
                assert!((peak-expected).abs() < 0.0001,"gain {gain}: peak {peak}, expected {expected}");
                let clips = outputs["clipped_samples"]["value"].as_f64().unwrap();
                assert_eq!(clips > 0.0,gain == 12.0);
                assert!((run["peak_delta"].as_f64().unwrap()-(peak-0.4)).abs() < 1e-12);
                assert!((run["distance"].as_f64().unwrap()-(peak-0.4).abs()).abs() < 1e-12);
                assert!(outputs["file"]["value"].as_str().unwrap().replace('\\',"/").ends_with(&path));
                let ai_bytes = std::fs::read(context.spaces.ai_root.join(&path)).unwrap();
                assert!(ai_bytes.starts_with(b"RIFF"));
                let child = store.load(&context.spaces,&app_data,run["render"]["run_id"].as_str().unwrap()).unwrap();
                assert_eq!(child.configuration["graph"]["nodes"][1]["parameters"]["gain_db"].as_f64(),Some(gain));
                assert!(child.files.iter().any(|file| file.space == "ai" && file.path == path && file.role == "output" && file.sha256.is_some()));
                if count == 3 && gain == 0.0 {
                    assert_eq!(std::fs::read(context.spaces.user_root.join(&path)).unwrap(),ai_bytes);
                } else {
                    assert!(!context.spaces.user_root.join(&path).exists());
                }
            }
            let export_count = report["trace"].as_array().unwrap().iter()
                .filter(|entry| entry["id"] == "export").count();
            let selected = &report["outputs"]["selected"];
            if count == 3 {
                assert_eq!(report["tool_calls"],6);
                assert_eq!(selected["found"],true);
                assert_eq!(selected["item"],runs[1]);
                assert_eq!(selected["index"],1);
                assert_eq!(selected["score"],runs[1]["distance"]);
                assert_eq!(report["step_results"]["eligible"],json!([runs[0],runs[1]]));
                assert!(runs[1]["distance"].as_f64().unwrap() < runs[0]["distance"].as_f64().unwrap());
                assert_eq!(report["outputs"]["delivery"]["branch"],"then");
                assert_eq!(report["outputs"]["delivery"]["steps"]["export"]["path"],"select-gain-001/gain-0.wav");
                assert_eq!(export_count,1);
                assert_eq!(std::fs::read_dir(context.spaces.user_root.join("select-gain-001")).unwrap().count(),1);
            } else {
                assert_eq!(report["tool_calls"],3);
                assert_eq!(*selected,json!({"found":false,"item":null,"index":null,"score":null}));
                assert_eq!(report["step_results"]["eligible"],json!([]));
                assert_eq!(report["outputs"]["delivery"],json!({"branch":"else","steps":{}}));
                assert_eq!(export_count,0);
                assert!(!context.spaces.user_root.join("select-gain-001").exists());
            }
            assert_eq!(std::fs::read_dir(context.spaces.ai_root.join("select-gain-001")).unwrap().count(),count+1);
            assert_eq!(std::fs::read(context.spaces.user_root.join("input.wav")).unwrap(),wav);
            assert_eq!(std::fs::read(context.spaces.ai_root.join("select-gain-001/input.wav")).unwrap(),wav);
            assert!(owned.shutdown_complete.load(Ordering::Acquire));
            shared.shutdown().unwrap();
            drop(context);
            std::fs::remove_dir_all(root).unwrap();
        }
    });
}

#[cfg(windows)]
#[test]
fn real_cpp_computed_gain_workflow_measures_and_exports_every_candidate() {
    use crate::backend::{Connection,DisconnectCallback};
    tauri::async_runtime::block_on(async {
        let (root,spaces) = spaces();
        let executable_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let executable = [executable_root.join("build/Debug/control-cli.exe"),executable_root.join("build/Release/control-cli.exe")]
            .into_iter().find(|path| path.is_file()).expect("Build the C++ control-cli before running integration tests");
        let mut wav = vec![
            b'R',b'I',b'F',b'F',44,0,0,0,b'W',b'A',b'V',b'E',b'f',b'm',b't',b' ',
            16,0,0,0,1,0,1,0,0x44,0xac,0,0,0x88,0x58,1,0,2,0,16,0,
            b'd',b'a',b't',b'a',8,0,0,0,
        ];
        for sample in [4000_i16,-4000,1000,-1000] { wav.extend_from_slice(&sample.to_le_bytes()); }
        std::fs::write(spaces.user_root.join("input.wav"),&wav).unwrap();
        let program = include_str!("../../../../examples/workflows/computed-gain.workflow.json");
        std::fs::write(spaces.user_root.join("computed.workflow.json"),program).unwrap();
        let callback: DisconnectCallback = Arc::new(|_| {});
        let shared = Arc::new(BackendManager::with_test_connection(Connection::spawn(
            &executable,spaces.user_root.clone(),"computed-user".into(),false,false,callback.clone()).unwrap()));
        let owned = Arc::new(BackendManager::with_test_connection(Connection::spawn(
            &executable,spaces.ai_root.clone(),"computed-ai".into(),false,false,callback).unwrap()));
        let store = Arc::new(RunStore::default());
        let app_data = root.join("data");
        let mut context = ToolContext::new(spaces,shared.clone(),"computed-user".into())
            .with_records(store.clone(),app_data.clone());
        context.owned = Some((owned.clone(),"computed-ai".into()));
        let cancel = AtomicBool::new(false);
        let args = json!({"space":"user","path":"computed.workflow.json"});
        let validated = context.dispatch("workflow","workflow_validate",&args,&cancel).await.unwrap();
        assert_eq!(validated["valid"],true,"{validated}");
        assert!(!context.spaces.ai_root.join("computed-gain-001").exists());
        assert!(!context.spaces.user_root.join("computed-gain-001").exists());
        assert!(store.list(&context.spaces,&app_data).unwrap().records.is_empty());
        let report = context.dispatch("workflow","workflow_run",&args,&cancel).await.unwrap();
        assert_eq!(report["state"],"succeeded","{report}");
        assert_eq!(report["graph_runs"],3);
        assert_eq!(report["tool_calls"],8);
        assert_eq!(report["steps_executed"],25);
        let trace = report["trace"].as_array().unwrap();
        assert_eq!(trace.iter().filter(|entry| entry["step_type"] == "compute").count(),12);
        assert_eq!(trace.iter().filter(|entry| entry["step_type"] == "concat").count(),4);
        assert_eq!(report["outputs"]["folder"],"computed-gain-001");
        let runs = report["outputs"]["runs"].as_array().unwrap();
        assert_eq!(runs.len(),3);
        let mut snapshots = Vec::new();
        for (index,(run,gain)) in runs.iter().zip([-12.0_f64,-6.0,0.0]).enumerate() {
            assert_eq!(run["gain_db"].as_f64(),Some(gain));
            let path = format!("computed-gain-001/gain-{index}.wav");
            assert_eq!(run["output_path"],path);
            assert_eq!(run["export"]["path"],path);
            assert_eq!(run["render"]["state"],"succeeded");
            let outputs = &run["render"]["result"]["outputs"];
            let peak = outputs["peak"]["value"].as_f64().expect("Real C++ peak meter returns a number");
            let expected = (4000.0/32768.0)*10.0_f64.powf(gain/20.0);
            assert!((peak-expected).abs() < 0.0001,"gain {gain}: peak {peak}, expected {expected}");
            assert_eq!(outputs["clipped_samples"]["value"].as_f64(),Some(0.0));
            assert!(outputs["file"]["value"].as_str().unwrap().replace('\\',"/").ends_with(&path));
            let delta = run["peak_delta"].as_f64().unwrap();
            assert!((delta-(peak-0.25)).abs() < 1e-12);
            assert!((run["absolute_peak_difference"].as_f64().unwrap()-delta.abs()).abs() < 1e-12);
            let ai_bytes = std::fs::read(context.spaces.ai_root.join(&path)).unwrap();
            assert!(ai_bytes.starts_with(b"RIFF"));
            assert_eq!(std::fs::read(context.spaces.user_root.join(&path)).unwrap(),ai_bytes);
            let child = store.load(&context.spaces,&app_data,run["render"]["run_id"].as_str().unwrap()).unwrap();
            assert_eq!(child.configuration["graph"]["nodes"][1]["parameters"]["gain_db"].as_f64(),Some(gain));
            assert!(child.files.iter().any(|file| file.space == "ai" && file.path == path && file.role == "output" && file.sha256.is_some()));
            snapshots.push((path,ai_bytes));
        }
        assert_ne!(snapshots[0].1,snapshots[1].1);
        assert_ne!(snapshots[1].1,snapshots[2].1);
        assert_eq!(std::fs::read(context.spaces.user_root.join("input.wav")).unwrap(),wav);
        assert_eq!(std::fs::read(context.spaces.ai_root.join("computed-gain-001/input.wav")).unwrap(),wav);
        assert!(owned.shutdown_complete.load(Ordering::Acquire));
        let repeated = context.dispatch("workflow","workflow_run",&args,&cancel).await.unwrap();
        assert_eq!(repeated["state"],"failed");
        assert_eq!(repeated["graph_runs"],0);
        assert!(repeated["error"]["step_path"].as_str().unwrap().contains("import"),"{repeated}");
        for (path,bytes) in snapshots {
            assert_eq!(std::fs::read(context.spaces.ai_root.join(&path)).unwrap(),bytes);
            assert_eq!(std::fs::read(context.spaces.user_root.join(&path)).unwrap(),bytes);
        }
        assert_eq!(std::fs::read(context.spaces.user_root.join("input.wav")).unwrap(),wav);
        shared.shutdown().unwrap();
        drop(context);
        std::fs::remove_dir_all(root).unwrap();
    });
}

#[test]
fn plugin_run_provenance_is_selected_from_catalog_not_graph_or_private_paths() {
    let graph = json!({"nodes":[{"id":"a","type":"test.plugin.gain_v1","plugin":{"id":"forged"}},
        {"id":"b","type":"gain"}]});
    let catalog = json!({"data":{"nodes":[{"typeId":"test.plugin.gain_v1","plugin":{
        "id":"test.plugin","implementation_version":"0.1.0","package_sha256":"a".repeat(64),
        "abi":{"major":0,"minor":1},"capabilities":[{"id":"ag.whole_sync/1","version":1}],
        "private_root":"not-for-model","credential":"never-copy"}}, {"typeId":"gain"}]}});
    let refs = graph_plugin_refs(&graph,&catalog);
    assert_eq!(refs.as_array().unwrap().len(),1);
    assert_eq!(refs[0]["node_id"],"a");
    assert_eq!(refs[0]["plugin"]["id"],"test.plugin");
    assert!(!refs.to_string().contains("not-for-model"));
    assert!(!refs.to_string().contains("never-copy"));
    assert_eq!(graph_plugin_refs(&json!({"nodes":[{"id":"b","type":"gain"}]}),&catalog),json!([]));
}

#[test]
fn workflow_exports_share_one_folder_and_record_relative_paths() {
    tauri::async_runtime::block_on(async {
        let (root,spaces) = spaces();
        let store = Arc::new(RunStore::default());
        let app_data = root.join("data");
        let program = json!({"schema_version":1,"inputs":{},"steps":[
            {"id":"folder","type":"call","tool":"directory_create","args":{"path":"batch"}},
            {"id":"write_a","type":"call","tool":"file_write_text","args":{"path":"batch/a.txt","content":"first"}},
            {"id":"export_a","type":"call","tool":"file_export","args":{"path":"batch/a.txt","user_path":"batch-001/a.txt"}},
            {"id":"write_b","type":"call","tool":"file_write_text","args":{"path":"batch/b.txt","content":"second"}},
            {"id":"export_b","type":"call","tool":"file_export","args":{"path":"batch/b.txt","user_path":"batch-001/b.txt"}}
        ],"outputs":{"first":{"$ref":"/steps/export_a/path"},"second":{"$ref":"/steps/export_b/path"}}});
        std::fs::write(spaces.ai_root.join("export.workflow.json"), program.to_string()).unwrap();
        let mut context = ToolContext::new(spaces,Arc::new(BackendManager::default()),"unconnected".into())
            .with_records(store.clone(),app_data.clone());
        let cancel = AtomicBool::new(false);
        let result = context.dispatch("workflow","workflow_run",&json!({"space":"ai","path":"export.workflow.json"}),&cancel).await.unwrap();
        assert_eq!(result["state"],"succeeded","{result}");
        assert_eq!(result["outputs"]["first"],"batch-001/a.txt");
        assert_eq!(result["outputs"]["second"],"batch-001/b.txt");
        let record = store.load(&context.spaces,&app_data,result["run_id"].as_str().unwrap()).unwrap();
        for (name, content) in [("a.txt","first"),("b.txt","second")] {
            let path = format!("batch-001/{name}");
            assert_eq!(std::fs::read_to_string(context.spaces.user_root.join(&path)).unwrap(),content);
            assert!(record.files.iter().any(|file| file.space == "user" && file.path == path && file.sha256.is_some()));
        }
        let repeated = context.dispatch("workflow","workflow_run",&json!({"space":"ai","path":"export.workflow.json"}),&cancel).await.unwrap();
        assert_eq!(repeated["state"],"failed");
        assert_eq!(std::fs::read_to_string(context.spaces.user_root.join("batch-001/a.txt")).unwrap(),"first");
        drop(context);
        std::fs::remove_dir_all(root).unwrap();
    });
}

fn spaces() -> (std::path::PathBuf, ToolWorkspaces) {
    let root = std::env::temp_dir().join(format!("audioprocess-agent-tools-{}-{}", std::process::id(), NEXT.fetch_add(1,Ordering::Relaxed)));
    let user = root.join("user");
    let data = root.join("data");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    let spaces = ToolWorkspaces::new(&user,&data).unwrap();
    (root,spaces)
}

#[test]
fn registry_and_dispatch_agree_on_modes() {
    assert!(names("workflow").contains(&"audio_convert".to_owned()));
    assert!(!names("graph").contains(&"audio_convert".to_owned()));
    assert!(!names("graph").contains(&"graph_run".to_owned()));
    assert!(names("workflow").contains(&"graph_run".to_owned()));
    assert!(names("workflow").contains(&"workflow_validate".to_owned()));
    assert!(names("workflow").contains(&"workflow_run".to_owned()));
    assert!(!names("graph").contains(&"workflow_run".to_owned()));
    assert!(names("forged").is_empty());
    for mode in ["graph","workflow"] {
        for tool in ["runs_list","runs_read","runs_check_files"] {
            assert!(names(mode).contains(&tool.to_owned()));
        }
        let described: Vec<String> = definitions(mode).iter().map(|v| v["function"]["name"].as_str().unwrap().to_owned()).collect();
        assert_eq!(described,names(mode));
        assert!(definitions(mode).iter().all(|v| v["function"]["parameters"]["additionalProperties"] == false));
    }
}

#[test]
fn audio_conversion_tool_rejects_permission_expansion_before_execution() {
    tauri::async_runtime::block_on(async {
        let (root, spaces) = spaces();
        let mut context = ToolContext::new(spaces, Arc::new(BackendManager::default()), "unconnected".into());
        let args = json!({"source_space":"user","source_path":"input.mp3","path":"output.wav"});
        let cancel = AtomicBool::new(false);
        assert!(context.dispatch("graph", "audio_convert", &args, &cancel).await.unwrap_err().contains("not available"));
        for (key,value) in [("target_space",json!("user")),("ffmpeg_args",json!(["-y"])),("channels",json!(3)),("sample_rate",json!(0)),("sample_rate",json!("48000"))] {
            let mut bad = args.clone(); bad[key] = value;
            assert!(context.dispatch("workflow", "audio_convert", &bad, &cancel).await.is_err());
        }
        let program = json!({"schema_version":1,"inputs":{},"steps":[{"id":"convert","type":"call","tool":"audio_convert","args":args}],"outputs":{"audio":{"$ref":"/steps/convert/path"}}});
        assert!(workflow::validate(&program).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    });
}

#[cfg(windows)]
#[test]
#[ignore = "Requires prepared bundled FFmpeg; converts synthetic audio through the real AI/workflow tool adapter"]
fn audio_conversion_workflow_to_graph_roundtrip() {
    use crate::backend::{Connection, DisconnectCallback};
    tauri::async_runtime::block_on(async {
        let (root, spaces) = spaces();
        let source = spaces.user_root.join("source.wav");
        let rate=16000_u32; let frames=1600_u32; let size=frames*2;
        let mut wav=Vec::new(); wav.extend(b"RIFF"); wav.extend((36+size).to_le_bytes()); wav.extend(b"WAVEfmt ");
        wav.extend(16_u32.to_le_bytes()); wav.extend(1_u16.to_le_bytes()); wav.extend(1_u16.to_le_bytes());
        wav.extend(rate.to_le_bytes()); wav.extend((rate*2).to_le_bytes()); wav.extend(2_u16.to_le_bytes()); wav.extend(16_u16.to_le_bytes());
        wav.extend(b"data"); wav.extend(size.to_le_bytes());
        for i in 0..frames { wav.extend((((i as f32 * 0.1).sin()*5000.) as i16).to_le_bytes()); }
        std::fs::write(&source,&wav).unwrap();
        let ffmpeg=crate::audio_conversion::locate_ffmpeg(&spaces).unwrap();
        let mut encode=std::process::Command::new(ffmpeg);
        use std::os::windows::process::CommandExt;
        encode.creation_flags(0x08000000).args(["-hide_banner","-nostdin","-loglevel","error","-n","-i"])
            .arg(&source).args(["-c:a","aac"]).arg(spaces.user_root.join("input.m4a"));
        assert!(encode.status().unwrap().success());
        let original = std::fs::read(spaces.user_root.join("input.m4a")).unwrap();
        let executable=std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build/Debug/control-cli.exe");
        let callback:DisconnectCallback=Arc::new(|_|{});
        let shared=Arc::new(BackendManager::with_test_connection(Connection::spawn(&executable,spaces.user_root.clone(),"convert-e2e".into(),false,false,callback.clone()).unwrap()));
        let owned=Arc::new(BackendManager::with_test_connection(Connection::spawn(&executable,spaces.ai_root.clone(),"convert-ai".into(),false,false,callback).unwrap()));
        let mut context=ToolContext::new(spaces,shared.clone(),"convert-e2e".into());
        context.owned=Some((owned,"convert-ai".into()));
        let cancel=AtomicBool::new(false);
        let program=json!({"schema_version":1,"inputs":{},"steps":[
            {"id":"convert","type":"call","tool":"audio_convert","args":{"source_space":"user","source_path":"input.m4a","path":"converted.wav"}},
            {"id":"inspect","type":"call","tool":"audio_inspect","args":{"space":"ai","path":{"$ref":"/steps/convert/path"}}},
            {"id":"run","type":"call","tool":"graph_run","args":{"mode":"offline","graph":{"schema_version":1,
                "nodes":[{"id":"input","type":"wav_input","parameters":{"path":{"$ref":"/steps/convert/path"}}},{"id":"output","type":"wav_output","parameters":{"path":"processed.wav"}}],
                "connections":[{"from":{"node":"input","port":"audio"},"to":{"node":"output","port":"audio"}}],"exports":[]}}},
            {"id":"export","type":"call","tool":"file_export","args":{"path":"processed.wav","user_path":"delivered.wav"}}
        ],"outputs":{"converted":{"$ref":"/steps/convert"},"delivered":{"$ref":"/steps/export/path"}}});
        let report=workflow::run(&workflow::validate(&program).unwrap(),None,&mut context,&cancel).await;
        assert_eq!(report.state,"succeeded","{}",serde_json::to_string(&report).unwrap());
        assert_eq!(std::fs::read(context.spaces.user_root.join("input.m4a")).unwrap(),original);
        assert!(context.spaces.user_root.join("delivered.wav").is_file());
        assert!(!context.spaces.user_root.join("converted.wav").exists());
        for format in ["wav","flac","mp3","m4a"] {
            let path = format!("delivery.{format}");
            let result = context.dispatch("workflow", "audio_convert", &json!({"source_space":"ai","source_path":"processed.wav","path":path,"output_format":format}), &cancel).await.unwrap();
            assert_eq!(result["output_format"], format);
            assert!(context.spaces.ai_root.join(&path).is_file());
            assert!(!context.spaces.user_root.join(&path).exists());
        }
        assert!(context.dispatch("workflow","audio_convert",&json!({"source_space":"user","source_path":"input.m4a","path":"converted.wav"}),&cancel).await.is_err());
        context.shutdown().await.unwrap(); shared.shutdown().unwrap(); drop(context); drop(shared);
        std::fs::remove_dir_all(root).unwrap();
    });
}

#[test]
fn workspace_listing_registry_and_dispatch_accept_bounded_paging_in_both_modes() {
    tauri::async_runtime::block_on(async {
        let (root,spaces) = spaces();
        std::fs::write(spaces.ai_root.join("a.txt"),"a").unwrap();
        std::fs::write(spaces.ai_root.join("b.txt"),"b").unwrap();
        let mut tools = ToolContext::new(spaces,Arc::new(BackendManager::default()),"unconnected".into());
        for mode in ["graph","workflow"] {
            let definitions = definitions(mode);
            let listing = definitions.iter().find(|entry| entry["function"]["name"] == "workspace_list").unwrap();
            assert_eq!(listing["function"]["parameters"]["required"],json!(["space"]));
            assert_eq!(listing["function"]["parameters"]["properties"]["offset"]["maximum"],10000);
            assert_eq!(listing["function"]["parameters"]["properties"]["limit"]["maximum"],200);
            let value = tools.dispatch(mode,"workspace_list",&json!({"space":"ai","offset":1,"limit":1}),&AtomicBool::new(false)).await.unwrap();
            assert_eq!(value["entries"][0]["name"],"b.txt");
            assert_eq!(value["total"],2);
            assert_eq!(value["truncated"],false);
            assert!(tools.dispatch(mode,"workspace_list",&json!({"space":"ai","limit":0}),&AtomicBool::new(false)).await.is_err());
        }
        // Existing Workflow calls pass the new arguments through the same whitelist.
        let value = WorkflowHost::call(&mut tools,"workspace_list",json!({"space":"ai","offset":1,"limit":1}),
            &AtomicBool::new(false),Instant::now()+Duration::from_secs(2),"/steps/0:list").await.unwrap();
        assert_eq!(value["entries"][0]["name"],"b.txt");
        drop(tools);
        std::fs::remove_dir_all(root).unwrap();
    });
}

#[test]
fn history_tools_require_records_and_do_not_expand_workflow_calls() {
    tauri::async_runtime::block_on(async {
        let (root,spaces) = spaces();
        let mut context = ToolContext::new(spaces,Arc::new(BackendManager::default()),"unconnected".into());
        let cancel = AtomicBool::new(false);
        for mode in ["graph","workflow"] {
            for (tool,args) in [
                ("runs_list",json!({})),
                ("runs_read",json!({"id":"0".repeat(64),"section":"configuration"})),
                ("runs_check_files",json!({"id":"0".repeat(64)})),
            ] {
                let error = context.dispatch(mode,tool,&args,&cancel).await.unwrap_err();
                assert!(error.to_ascii_lowercase().contains("record") || error.to_ascii_lowercase().contains("history"),"{error}");
                assert!(!error.contains("Tool is not available in this mode"),"{error}");
                let program = json!({"schema_version":1,"inputs":{},"steps":[
                    {"id":"history","type":"call","tool":tool,"args":args}
                ],"outputs":{}});
                let path = format!("{tool}.workflow.json");
                std::fs::write(context.spaces.ai_root.join(&path),program.to_string()).unwrap();
                let error = context.dispatch("workflow","workflow_validate",&json!({"space":"ai","path":path}),&cancel).await.unwrap_err();
                assert!(error.contains("not allowed"),"{error}");
            }
        }
        context = context.with_records(Arc::new(RunStore::default()),root.join("data"));
        for tool in ["runs_list","runs_read","runs_check_files"] {
            let args = if tool == "runs_list" { json!({}) } else { json!({"id":"0".repeat(64)}) };
            let error = WorkflowHost::call(&mut context,tool,args,&cancel,
                Instant::now()+Duration::from_secs(1),"/steps/0").await.unwrap_err();
            assert!(error.contains("not allowed") || error.contains("not available") || error.contains("Unknown tool"),"{error}");
        }
        drop(context);
        std::fs::remove_dir_all(root).unwrap();
    });
}

#[cfg(windows)]
fn feedback_gain_graph(gain_db: i32, output: &str) -> Value {
    json!({"schema_version":1,"nodes":[
        {"id":"input","type":"wav_input","parameters":{"path":"input.wav"}},
        {"id":"gain","type":"gain","parameters":{"gain_db":gain_db}},
        {"id":"meter","type":"peak_meter"},
        {"id":"output","type":"wav_output","parameters":{"path":output}}
    ],"connections":[
        {"from":{"node":"input","port":"audio"},"to":{"node":"gain","port":"audio"}},
        {"from":{"node":"gain","port":"audio"},"to":{"node":"meter","port":"audio"}},
        {"from":{"node":"gain","port":"audio"},"to":{"node":"output","port":"audio"}}
    ],"exports":[{"name":"file","node":"output","port":"path"},{"name":"peak","node":"meter","port":"peak"}]})
}

#[cfg(windows)]
#[test]
fn manual_workflow_snapshot_owns_ai_space_graph_child_and_confirms_cleanup() {
    use crate::backend::{Connection,DisconnectCallback};
    tauri::async_runtime::block_on(async {
        let (root,spaces) = spaces();
        let executable_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let executable = [executable_root.join("build/Debug/control-cli.exe"),executable_root.join("build/Release/control-cli.exe")]
            .into_iter().find(|path| path.is_file()).expect("Build C++ control-cli before integration tests");
        let mut wav = vec![
            b'R',b'I',b'F',b'F',44,0,0,0,b'W',b'A',b'V',b'E',b'f',b'm',b't',b' ',
            16,0,0,0,1,0,1,0,0x44,0xac,0,0,0x88,0x58,1,0,2,0,16,0,
            b'd',b'a',b't',b'a',8,0,0,0,
        ];
        for sample in [4000_i16,-4000,1000,-1000] { wav.extend_from_slice(&sample.to_le_bytes()); }
        std::fs::write(spaces.ai_root.join("input.wav"),wav).unwrap();
        let callback: DisconnectCallback = Arc::new(|_| {});
        let shared = Arc::new(BackendManager::with_test_connection(Connection::spawn(
            &executable,spaces.user_root.clone(),"editor-user".into(),false,false,callback.clone()).unwrap()));
        let owned = Arc::new(BackendManager::with_test_connection(Connection::spawn(
            &executable,spaces.ai_root.clone(),"editor-ai".into(),false,false,callback).unwrap()));
        let store = Arc::new(RunStore::default());
        let data = root.join("data");
        let manager = crate::agent_runtime::AgentManager::default();
        let (cancel,lease) = manager.begin_manual("editor-cpp").unwrap();
        let mut context = ToolContext::new(spaces,shared.clone(),"editor-user".into())
            .with_records(store.clone(),data.clone()).with_manual_origin().with_failure_sink(manager.failure_sink());
        context.owned = Some((owned.clone(),"editor-ai".into()));
        let document = json!({"schema_version":1,"inputs":{},"steps":[
            {"id":"gain","type":"call","tool":"graph_run","args":{"mode":"offline","graph":feedback_gain_graph(-6,"manual-result.wav")}},
            {"id":"deliver","type":"call","tool":"file_export","args":{"path":"manual-result.wav","user_path":"manual-batch/result.wav"}}
        ],"outputs":{"graph":{"$ref":"/steps/gain"},"file":{"$ref":"/steps/deliver"}}});
        let text = document.to_string();
        let result = context.execute_workflow_snapshot(workflow::validate(&document).unwrap(),document,
            json!({"kind":"editor","sha256":format!("{:x}",Sha256::digest(text.as_bytes()))}),
            "Workflow editor run",None,vec![],&cancel).await.unwrap();
        assert_eq!(result["state"],"succeeded","{result}");
        assert!(owned.shutdown_complete.load(Ordering::Acquire));
        let parent = result["run_id"].as_str().unwrap();
        let child = result["outputs"]["graph"]["run_id"].as_str().unwrap();
        let parent_record = store.load(&context.spaces,&data,parent).unwrap();
        let child_record = store.load(&context.spaces,&data,child).unwrap();
        assert_eq!(parent_record.origin,"manual");
        assert_eq!(child_record.origin,"manual");
        assert_eq!(child_record.parent_id.as_deref(),Some(parent));
        assert_eq!(child_record.configuration["file_space"],"ai");
        assert_eq!(child_record.configuration["workflow_step_path"],"/steps/0:gain");
        assert!(context.spaces.ai_root.join("manual-result.wav").is_file());
        assert!(!context.spaces.user_root.join("manual-result.wav").exists());
        assert!(context.spaces.user_root.join("manual-batch/result.wav").is_file());
        assert!(child_record.files.iter().all(|file| file.space == "ai"));
        shared.shutdown().unwrap();
        drop(context);
        drop(lease);
        manager.wait_idle(Duration::from_millis(1)).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    });
}

#[cfg(windows)]
#[test]
fn real_cpp_history_feedback_creates_a_new_run_and_preserves_the_baseline() {
    use crate::backend::{Connection,DisconnectCallback};
    tauri::async_runtime::block_on(async {
        let (root,spaces) = spaces();
        let executable_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let executable = [executable_root.join("build/Debug/control-cli.exe"),executable_root.join("build/Release/control-cli.exe")]
            .into_iter().find(|path| path.is_file()).expect("Build the C++ control-cli before running integration tests");
        let mut wav = vec![
            b'R',b'I',b'F',b'F',44,0,0,0,b'W',b'A',b'V',b'E',b'f',b'm',b't',b' ',
            16,0,0,0,1,0,1,0,0x44,0xac,0,0,0x88,0x58,1,0,2,0,16,0,
            b'd',b'a',b't',b'a',8,0,0,0,
        ];
        for sample in [4000_i16,-4000,1000,-1000] { wav.extend_from_slice(&sample.to_le_bytes()); }
        std::fs::write(spaces.user_root.join("input.wav"),&wav).unwrap();
        let callback: DisconnectCallback = Arc::new(|_| {});
        let shared = Arc::new(BackendManager::with_test_connection(Connection::spawn(
            &executable,spaces.user_root.clone(),"history-user".into(),false,false,callback.clone()).unwrap()));
        let store = Arc::new(RunStore::default());
        let app_data = root.join("data");
        let mut context = ToolContext::new(spaces,shared.clone(),"history-user".into())
            .with_records(store.clone(),app_data.clone());
        let cancel = AtomicBool::new(false);
        context.dispatch("workflow","file_copy_to_ai",&json!({"source_space":"user","source_path":"input.wav","path":"input.wav"}),&cancel).await.unwrap();
        let baseline_graph = feedback_gain_graph(-12,"baseline.wav");
        let baseline_owned = Arc::new(BackendManager::with_test_connection(Connection::spawn(
            &executable,context.spaces.ai_root.clone(),"history-baseline".into(),false,false,callback.clone()).unwrap()));
        context.owned = Some((baseline_owned.clone(),"history-baseline".into()));
        let baseline = context.dispatch("workflow","graph_run",&json!({"mode":"offline","graph":baseline_graph}),&cancel).await.unwrap();
        assert_eq!(baseline["state"],"succeeded","{baseline}");
        assert!(baseline_owned.shutdown_complete.load(Ordering::Acquire));
        let baseline_id = baseline["run_id"].as_str().expect("Recorded Graph result includes run_id").to_owned();
        let detail_before = serde_json::to_value(store.load(&context.spaces,&app_data,&baseline_id).unwrap()).unwrap();
        let identity = context.spaces.user_root.to_string_lossy().to_lowercase();
        let snapshot_path = app_data.join("run-records/v1").join(format!("{:x}",Sha256::digest(identity.as_bytes())))
            .join("records").join(format!("{baseline_id}.json"));
        let snapshot_before = std::fs::read(&snapshot_path).unwrap();
        let audio_before = std::fs::read(context.spaces.ai_root.join("baseline.wav")).unwrap();

        let listed = context.dispatch("graph","runs_list",&json!({"kind":"graph","state":"succeeded"}),&cancel).await.unwrap();
        assert_eq!(listed["records"].as_array().unwrap().len(),1);
        assert_eq!(listed["records"][0]["id"],baseline_id);
        let configuration = context.dispatch("graph","runs_read",&json!({"id":baseline_id,"section":"configuration"}),&cancel).await.unwrap();
        let previous_result = context.dispatch("graph","runs_read",&json!({"id":baseline_id,"section":"result"}),&cancel).await.unwrap();
        assert_eq!(configuration["data"],detail_before["configuration"]);
        assert_eq!(previous_result["data"],detail_before["result"]);
        assert_eq!(configuration["data"]["graph"],baseline_graph);
        let checks = context.dispatch("graph","runs_check_files",&json!({"id":baseline_id}),&cancel).await.unwrap();
        assert!(checks["files"].as_array().unwrap().iter().all(|file| file["status"] == "available"));
        assert!(context.dispatch("graph","graph_run",&json!({"mode":"offline","graph":baseline_graph}),&cancel).await.is_err());

        // Deterministic feedback changes a copy of the recorded Graph, never its snapshot.
        let mut adjusted_graph = configuration["data"]["graph"].clone();
        adjusted_graph["nodes"][1]["parameters"]["gain_db"] = json!(-6);
        adjusted_graph["nodes"][3]["parameters"]["path"] = json!("feedback.wav");
        let adjusted_owned = Arc::new(BackendManager::with_test_connection(Connection::spawn(
            &executable,context.spaces.ai_root.clone(),"history-adjusted".into(),false,false,callback).unwrap()));
        context.owned = Some((adjusted_owned.clone(),"history-adjusted".into()));
        let adjusted = context.dispatch("workflow","graph_run",&json!({"mode":"offline","graph":adjusted_graph}),&cancel).await.unwrap();
        assert_eq!(adjusted["state"],"succeeded","{adjusted}");
        assert!(adjusted_owned.shutdown_complete.load(Ordering::Acquire));
        let adjusted_id = adjusted["run_id"].as_str().unwrap();
        assert_ne!(adjusted_id,baseline_id);
        let adjusted_detail = store.load(&context.spaces,&app_data,adjusted_id).unwrap();
        assert_eq!(adjusted_detail.configuration["graph"],adjusted_graph);
        assert_eq!(adjusted_detail.state,"succeeded");
        assert!(adjusted_detail.files.iter().any(|file| file.role == "output" && file.path == "feedback.wav" && file.sha256.is_some()));
        assert_eq!(store.list(&context.spaces,&app_data).unwrap().records.len(),2);
        assert_ne!(std::fs::read(context.spaces.ai_root.join("feedback.wav")).unwrap(),audio_before);
        assert_eq!(std::fs::read(context.spaces.ai_root.join("baseline.wav")).unwrap(),audio_before);
        assert_eq!(std::fs::read(context.spaces.ai_root.join("input.wav")).unwrap(),wav);
        assert_eq!(std::fs::read(context.spaces.user_root.join("input.wav")).unwrap(),wav);
        assert_eq!(std::fs::read(snapshot_path).unwrap(),snapshot_before);
        assert_eq!(serde_json::to_value(store.load(&context.spaces,&app_data,&baseline_id).unwrap()).unwrap(),detail_before);
        shared.shutdown().unwrap();
        drop(context);
        std::fs::remove_dir_all(root).unwrap();
    });
}

#[cfg(windows)]
#[test]
fn normalizes_verbatim_and_plain_absolute_workspace_paths() {
    let (root,spaces) = spaces();
    let absolute = spaces.user_root.join("input.wav").to_string_lossy().into_owned();
    let plain = absolute.strip_prefix(r"\\?\").unwrap_or(&absolute).to_owned();
    let files = normalize_file_drafts(&spaces,vec![
        RunFileDraft { space:"user".into(),path:absolute,role:"input".into() },
        RunFileDraft { space:"user".into(),path:plain,role:"input".into() },
    ]);
    assert_eq!(files.len(),1);
    assert_eq!(files[0].path,"input.wav");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_metadata_cannot_turn_an_oversized_report_into_success() {
    let report = json!({"schema_version":1,"state":"succeeded","outputs":{},"step_results":{},
        "trace":[{"result":"x".repeat(1024 * 1024)}],"steps_executed":1,"tool_calls":1,"graph_runs":0});
    let source = json!({"space":"ai","path":"test.workflow.json","sha256":"0".repeat(64)});
    let bounded = attach_workflow_source(report,source.clone());
    assert_eq!(bounded["state"],"limited");
    assert_eq!(bounded["error"]["code"],"result_limit");
    assert_eq!(bounded["source"],source);
    assert_eq!(bounded["steps_executed"],1);
    assert!(serde_json::to_vec(&bounded).unwrap().len() <= 1024 * 1024);
}

#[test]
fn file_tool_roundtrip_and_forged_run_rejected_before_backend() {
    tauri::async_runtime::block_on(async {
    let (root,spaces) = spaces();
    let mut context = ToolContext::new(spaces,Arc::new(BackendManager::default()),"unconnected".into());
    let cancel = AtomicBool::new(false);
    let directory = context.dispatch("graph","directory_create",&json!({"path":"notes"}),&cancel).await.unwrap();
    assert_eq!(directory["space"],"ai");
    assert!(context.dispatch("graph","directory_create",&json!({"path":"notes/two/three/four"}),&cancel).await.is_err());
    let written = context.dispatch("graph","file_write_text",&json!({"path":"note.txt","content":"hello"}),&cancel).await.unwrap();
    assert_eq!(written["path"],"note.txt");
    let read = context.dispatch("graph","file_read_text",&json!({"space":"ai","path":"note.txt"}),&cancel).await.unwrap();
    assert_eq!(read["content"],"hello");
    assert!(context.dispatch("graph","graph_run",&json!({"mode":"offline","graph":{}}),&cancel).await.is_err());
    assert!(context.dispatch("graph","workflow_validate",&json!({"space":"ai","path":"note.txt"}),&cancel).await.is_err());
    assert!(context.dispatch("graph","workflow_run",&json!({"space":"ai","path":"note.txt"}),&cancel).await.is_err());
    assert!(context.dispatch("forged","file_read_text",&json!({"space":"ai","path":"note.txt"}),&cancel).await.is_err());
    assert!(context.dispatch("graph","file_write_text",&json!({"path":"note.txt","content":"x","extra":1}),&cancel).await.is_err());
    cancel.store(true,Ordering::Release);
    assert!(context.dispatch("graph","file_delete",&json!({"path":"note.txt"}),&cancel).await.is_err());
    assert!(root.join("user").is_dir());
    std::fs::remove_dir_all(root).unwrap();
    });
}

#[test]
fn workflow_file_is_snapshotted_strictly_and_validation_has_no_effects() {
    tauri::async_runtime::block_on(async {
        let (root,spaces) = spaces();
        let mut context = ToolContext::new(spaces,Arc::new(BackendManager::default()),"unconnected".into());
        let cancel = AtomicBool::new(false);
        let program = json!({"schema_version":1,"inputs":{"choice":2},"steps":[
            {"id":"write","type":"call","tool":"file_write_text","args":{"path":"should-not-exist.txt","content":"written"}},
            {"id":"branch","type":"if","condition":{"op":"eq","left":{"$ref":"/inputs/choice"},"right":2},
                "then":[{"id":"chosen","type":"set","value":"yes"}],"else":[{"id":"chosen","type":"set","value":"no"}]}
        ],"outputs":{"branch":{"$ref":"/steps/branch"}}});
        std::fs::write(context.spaces.user_root.join("program.workflow.json"),program.to_string()).unwrap();
        let args = json!({"space":"user","path":"program.workflow.json"});
        let validated = context.dispatch("workflow","workflow_validate",&args,&cancel).await.unwrap();
        assert_eq!(validated["source"]["space"],"user");
        assert_eq!(validated["source"]["path"],"program.workflow.json");
        assert_eq!(validated["source"]["sha256"].as_str().unwrap().len(),64);
        assert!(!context.spaces.ai_root.join("should-not-exist.txt").exists());
        let run = context.dispatch("workflow","workflow_run",&json!({"space":"user","path":"program.workflow.json"}),&cancel).await.unwrap();
        assert_eq!(run["state"],"succeeded");
        assert_eq!(run["outputs"]["branch"]["branch"],"then");
        assert_eq!(std::fs::read_to_string(context.spaces.ai_root.join("should-not-exist.txt")).unwrap(),"written");
        assert_eq!(run["source"],validated["source"]);
        assert!(context.dispatch("workflow","workflow_run",&json!({"space":"user","path":"program.workflow.json","inputs":{"undeclared":1}}),&cancel).await.unwrap()["state"] != "succeeded");
        std::fs::write(context.spaces.user_root.join("duplicate.workflow.json"),
            "{\"schema_version\":1,\"schema_version\":1,\"steps\":[],\"outputs\":{}}").unwrap();
        assert!(context.dispatch("workflow","workflow_validate",&json!({"space":"user","path":"duplicate.workflow.json"}),&cancel).await.is_err());
        let recursive = json!({"schema_version":1,"inputs":{},"steps":[
            {"id":"again","type":"call","tool":"workflow_run","args":{"space":"user","path":"recursive.workflow.json"}}
        ],"outputs":{}});
        std::fs::write(context.spaces.user_root.join("recursive.workflow.json"),recursive.to_string()).unwrap();
        assert!(context.dispatch("workflow","workflow_validate",&json!({"space":"user","path":"recursive.workflow.json"}),&cancel).await.is_err());
        std::fs::write(context.spaces.user_root.join("oversize.workflow.json")," ".repeat(64 * 1024 + 1)).unwrap();
        assert!(context.dispatch("workflow","workflow_validate",&json!({"space":"user","path":"oversize.workflow.json"}),&cancel).await.is_err());
        assert!(context.dispatch("workflow","workflow_validate",&json!({"space":"user","path":"../escape.json"}),&cancel).await.is_err());
        assert!(context.dispatch("workflow","workflow_validate",&json!({"space":"user","path":"program.workflow.json","inputs":{}}),&cancel).await.is_err());
        drop(context);
        std::fs::remove_dir_all(root).unwrap();
    });
}

#[test]
fn workflow_failure_keeps_completed_effects_and_skips_later_calls() {
    tauri::async_runtime::block_on(async {
        let (root,spaces) = spaces();
        let mut context = ToolContext::new(spaces,Arc::new(BackendManager::default()),"unconnected".into());
        let cancel = AtomicBool::new(false);
        let program = json!({"schema_version":1,"inputs":{},"steps":[
            {"id":"before","type":"call","tool":"file_write_text","args":{"path":"before.txt","content":"yes"}},
            {"id":"missing","type":"call","tool":"file_read_text","args":{"space":"ai","path":"missing.txt"}},
            {"id":"after","type":"call","tool":"file_write_text","args":{"path":"after.txt","content":"no"}}
        ],"outputs":{"last":{"$ref":"/steps/after"}}});
        std::fs::write(context.spaces.ai_root.join("failure.workflow.json"),program.to_string()).unwrap();
        let report = context.dispatch("workflow","workflow_run",&json!({"space":"ai","path":"failure.workflow.json"}),&cancel).await.unwrap();
        assert_eq!(report["state"],"failed");
        assert!(context.spaces.ai_root.join("before.txt").is_file());
        assert!(!context.spaces.ai_root.join("after.txt").exists());
        assert!(report["error"].is_object());
        assert!(report["trace"].is_array());
        drop(context);
        std::fs::remove_dir_all(root).unwrap();
    });
}

#[cfg(windows)]
#[test]
fn real_cpp_graph_run_imports_and_exports_without_overwriting() {
    use crate::backend::{Connection,DisconnectCallback};
    tauri::async_runtime::block_on(async {
        let (root,spaces) = spaces();
        let executable_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let executable = [executable_root.join("build/Debug/control-cli.exe"), executable_root.join("build/Release/control-cli.exe")]
            .into_iter().find(|path| path.is_file()).expect("Build the C++ control-cli before running integration tests");
        let wav: [u8;48] = [
            b'R',b'I',b'F',b'F',40,0,0,0,b'W',b'A',b'V',b'E',b'f',b'm',b't',b' ',
            16,0,0,0,1,0,1,0,0x44,0xac,0,0,0x88,0x58,1,0,2,0,16,0,
            b'd',b'a',b't',b'a',4,0,0,0,0,0,0,0,
        ];
        std::fs::write(spaces.user_root.join("input.wav"),wav).unwrap();
        let callback: DisconnectCallback = Arc::new(|_| {});
        let user_connection = Connection::spawn(&executable,spaces.user_root.clone(),"agent-user".into(),false,false,callback.clone()).unwrap();
        let ai_connection = Connection::spawn(&executable,spaces.ai_root.clone(),"agent-ai".into(),false,false,callback).unwrap();
        let shared = Arc::new(BackendManager::with_test_connection(user_connection));
        let owned = Arc::new(BackendManager::with_test_connection(ai_connection));
        let mut context = ToolContext::new(spaces,shared.clone(),"agent-user".into());
        context.owned = Some((owned.clone(),"agent-ai".into()));
        let cancel = AtomicBool::new(false);
        context.dispatch("workflow","file_copy_to_ai",&json!({"source_space":"user","source_path":"input.wav","path":"input.wav"}),&cancel).await.unwrap();
        let graph = json!({"schema_version":1,"nodes":[
            {"id":"input","type":"wav_input","parameters":{"path":"input.wav"}},
            {"id":"output","type":"wav_output","parameters":{"path":"output.wav"}}
        ],"connections":[{"from":{"node":"input","port":"audio"},"to":{"node":"output","port":"audio"}}],
        "exports":[{"name":"file","node":"output","port":"path"}]});
        let result = context.dispatch("workflow","graph_run",&json!({"mode":"offline","graph":graph}),&cancel).await.unwrap();
        assert_eq!(result["state"],"succeeded");
        assert!(context.spaces.ai_root.join("output.wav").is_file());
        assert!(owned.shutdown_complete.load(Ordering::Acquire));
        context.dispatch("workflow","file_export",&json!({"path":"output.wav","user_path":"output.wav"}),&cancel).await.unwrap();
        assert!(context.spaces.user_root.join("output.wav").is_file());
        assert!(context.dispatch("workflow","file_export",&json!({"path":"output.wav","user_path":"output.wav"}),&cancel).await.is_err());
        shared.shutdown().unwrap();
        drop(context);
        std::fs::remove_dir_all(root).unwrap();
    });
}

#[cfg(windows)]
#[test]
fn real_cpp_workflow_runs_three_gain_variants_and_preserves_source() {
    use crate::backend::{Connection,DisconnectCallback};
    tauri::async_runtime::block_on(async {
        let (root,spaces) = spaces();
        let executable_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let executable = [executable_root.join("build/Debug/control-cli.exe"), executable_root.join("build/Release/control-cli.exe")]
            .into_iter().find(|path| path.is_file()).expect("Build the C++ control-cli before running integration tests");
        let mut wav: Vec<u8> = vec![
            b'R',b'I',b'F',b'F',44,0,0,0,b'W',b'A',b'V',b'E',b'f',b'm',b't',b' ',
            16,0,0,0,1,0,1,0,0x44,0xac,0,0,0x88,0x58,1,0,2,0,16,0,
            b'd',b'a',b't',b'a',8,0,0,0,
        ];
        for sample in [4000_i16,-4000,1000,-1000] { wav.extend_from_slice(&sample.to_le_bytes()); }
        std::fs::write(spaces.user_root.join("input.wav"),&wav).unwrap();
        let example = executable_root.join("examples/workflows/gain-comparison.workflow.json");
        let program = std::fs::read(&example).unwrap();
        std::fs::write(spaces.user_root.join("gain.workflow.json"),program).unwrap();
        let callback: DisconnectCallback = Arc::new(|_| {});
        let user_connection = Connection::spawn(&executable,spaces.user_root.clone(),"workflow-user".into(),false,false,callback.clone()).unwrap();
        let ai_connection = Connection::spawn(&executable,spaces.ai_root.clone(),"workflow-ai".into(),false,false,callback).unwrap();
        let shared = Arc::new(BackendManager::with_test_connection(user_connection));
        let owned = Arc::new(BackendManager::with_test_connection(ai_connection));
        let app_data = root.join("data");
        let store = Arc::new(RunStore::default());
        let mut context = ToolContext::new(spaces,shared.clone(),"workflow-user".into())
            .with_records(store.clone(),app_data.clone());
        context.owned = Some((owned.clone(),"workflow-ai".into()));
        let cancel = AtomicBool::new(false);
        let args = json!({"space":"user","path":"gain.workflow.json"});
        let validated = context.dispatch("workflow","workflow_validate",&args,&cancel).await.unwrap();
        assert_eq!(validated["valid"],true);
        assert!(!context.spaces.ai_root.join("workflow-input.wav").exists());
        let report = context.dispatch("workflow","workflow_run",&args,&cancel).await.unwrap();
        assert_eq!(report["state"],"succeeded","{report}");
        assert_eq!(report["graph_runs"],3);
        assert_eq!(report["outputs"]["runs"].as_array().unwrap().len(),3);
        assert_eq!(std::fs::read(context.spaces.user_root.join("input.wav")).unwrap(),wav);
        let output_paths = ["workflow-gain-minus12.wav","workflow-gain-minus6.wav","workflow-gain-zero.wav"];
        let outputs: Vec<_> = output_paths.iter().map(|path| std::fs::read(context.spaces.ai_root.join(path)).unwrap()).collect();
        assert!(outputs.iter().all(|output| output.starts_with(b"RIFF")));
        assert_ne!(outputs[0],outputs[1]);
        assert_ne!(outputs[1],outputs[2]);
        let records = store.list(&context.spaces,&app_data).unwrap().records;
        assert_eq!(records.len(),4);
        let parent = records.iter().find(|record| record.kind == "workflow").unwrap();
        assert_eq!(parent.state,"succeeded");
        let parent_detail = store.load(&context.spaces,&app_data,&parent.id).unwrap();
        assert_eq!(parent_detail.configuration["workflow"]["schema_version"],1);
        let children: Vec<_> = records.iter().filter(|record| record.parent_id.as_deref() == Some(parent.id.as_str())).collect();
        assert_eq!(children.len(),3);
        for child in children {
            let detail = store.load(&context.spaces,&app_data,&child.id).unwrap();
            assert!(detail.configuration["workflow_step_path"].as_str().is_some());
            assert!(detail.files.iter().any(|file| file.role == "input" && file.sha256.is_some()));
            assert!(detail.files.iter().any(|file| file.role == "output" && file.sha256.is_some()));
        }
        assert!(owned.shutdown_complete.load(Ordering::Acquire));
        let repeated = context.dispatch("workflow","workflow_run",&args,&cancel).await.unwrap();
        assert_eq!(repeated["state"],"failed");
        shared.shutdown().unwrap();
        drop(context);
        std::fs::remove_dir_all(root).unwrap();
    });
}
