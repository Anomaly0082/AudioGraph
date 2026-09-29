use super::*;
use std::sync::atomic::AtomicU64;

static NEXT: AtomicU64 = AtomicU64::new(0);

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
    assert!(!names("graph").contains(&"graph_run".to_owned()));
    assert!(names("workflow").contains(&"graph_run".to_owned()));
    assert!(names("workflow").contains(&"workflow_validate".to_owned()));
    assert!(names("workflow").contains(&"workflow_run".to_owned()));
    assert!(!names("graph").contains(&"workflow_run".to_owned()));
    assert!(names("forged").is_empty());
    for mode in ["graph","workflow"] {
        let described: Vec<String> = definitions(mode).iter().map(|v| v["function"]["name"].as_str().unwrap().to_owned()).collect();
        assert_eq!(described,names(mode));
        assert!(definitions(mode).iter().all(|v| v["function"]["parameters"]["additionalProperties"] == false));
    }
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
