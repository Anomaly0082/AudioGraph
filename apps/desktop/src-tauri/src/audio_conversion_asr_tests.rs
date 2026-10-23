//! Opt-in conversion integration with a real, separately installed CPU ASR plugin.
//! No user recordings, devices, network downloads or model API requests.
use super::*;
use crate::backend::{Connection, DisconnectCallback};
use crate::plugin_manager::PluginManager;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

struct Cleanup {
    root: PathBuf,
    managers: Vec<Arc<BackendManager>>,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        for manager in &self.managers { let _ = manager.shutdown(); }
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
#[ignore = "Requires explicit trusted ASR package, pinned public Chinese WAV and bundled FFmpeg; CPU inference only"]
fn audio_conversion_four_formats_to_real_asr_workflow() {
    let package = PathBuf::from(std::env::var_os("AUDIOPROCESS_ASR_PACKAGE").expect("Supply trusted ASR package"));
    let sample = PathBuf::from(std::env::var_os("AUDIOPROCESS_ASR_WAV").expect("Supply pinned public Chinese WAV"));
    let original = fs::read(&sample).unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(&original)),
        "b77f1794fe374a0ba1ee1dc458bfaf9349496cbbfc32780c50ba3c5a7ad8e373");
    tauri::async_runtime::block_on(async {
        let (root, spaces) = spaces();
        let mut cleanup = Cleanup { root: root.clone(), managers: Vec::new() };
        fs::write(spaces.user_root.join("原始语音.wav"), &original).unwrap();
        let packages = root.join("plugins");
        let data = root.join("data");
        let installer = PluginManager::default();
        installer.initialize(&packages, &data).unwrap();
        installer.install(&fs::canonicalize(package).unwrap(), true).unwrap();
        let restarted = PluginManager::default();
        let snapshot = restarted.initialize(&packages, &data).unwrap();
        let executable = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build/Debug/control-cli.exe");
        let callback: DisconnectCallback = Arc::new(|_| {});
        let shared = Arc::new(BackendManager::with_test_connection(Connection::spawn_with_plugins(
            &executable, spaces.user_root.clone(), "conversion-asr-user".into(), false, false,
            callback.clone(), snapshot.clone()).unwrap()));
        cleanup.managers.push(shared.clone());
        let owned = Arc::new(BackendManager::with_test_connection(Connection::spawn_with_plugins(
            &executable, spaces.ai_root.clone(), "conversion-asr-ai".into(), false, false,
            callback, snapshot).unwrap()));
        cleanup.managers.push(owned.clone());
        let mut context = ToolContext::new(spaces, shared, "conversion-asr-user".into());
        context.owned = Some((owned, "conversion-asr-ai".into()));
        let cancel = AtomicBool::new(false);
        for format in ["wav", "flac", "mp3", "m4a"] {
            let input = format!("输入.{format}");
            let options = serde_json::from_value(json!({"output_format":format})).unwrap();
            crate::audio_conversion::convert(&context.spaces, "user", "原始语音.wav", "user", &input,
                options, &cancel, Instant::now() + Duration::from_secs(30)).unwrap();
            let input_bytes = fs::read(context.spaces.user_root.join(&input)).unwrap();
            let wav = format!("{format}-converted.wav");
            let transcript = format!("{format}-transcript.txt");
            let program = json!({"schema_version":1,"inputs":{},"steps":[
                {"id":"convert","type":"call","tool":"audio_convert","args":{
                    "source_space":"user","source_path":input,"path":wav,"sample_rate":16000,"channels":1}},
                {"id":"inspect","type":"call","tool":"audio_inspect","args":{
                    "space":"ai","path":{"$ref":"/steps/convert/path"}}},
                {"id":"transcribe","type":"call","tool":"graph_run","args":{"mode":"offline","graph":{
                    "schema_version":1,"nodes":[
                        {"id":"input","type":"wav_input","parameters":{"path":{"$ref":"/steps/convert/path"}}},
                        {"id":"asr","type":"org.audiograph.asr.whisper_zh_v1","parameters":{"threads":2}},
                        {"id":"output","type":"text_output","parameters":{"path":transcript}}
                    ],"connections":[
                        {"from":{"node":"input","port":"audio"},"to":{"node":"asr","port":"audio"}},
                        {"from":{"node":"asr","port":"text"},"to":{"node":"output","port":"text"}}
                    ],"exports":[{"name":"text","node":"asr","port":"text"}]}}},
                {"id":"export","type":"call","tool":"file_export","args":{"path":transcript,"user_path":transcript}}
            ],"outputs":{"text":{"$ref":"/steps/transcribe/result/outputs/text/value"}}});
            let report = workflow::run(&workflow::validate(&program).unwrap(), None, &mut context, &cancel).await;
            assert_eq!(report.state, "succeeded", "{format}: {}", serde_json::to_string(&report).unwrap());
            let text = report.outputs["text"].as_str().unwrap();
            assert!(text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)), "{format}: {text}");
            assert_eq!(fs::read(context.spaces.user_root.join(&transcript)).unwrap(), text.as_bytes());
            assert_eq!(fs::read(context.spaces.ai_root.join(&transcript)).unwrap(), text.as_bytes());
            assert_eq!(fs::read(context.spaces.user_root.join(&input)).unwrap(), input_bytes);
            assert!(!context.spaces.user_root.join(&wav).exists());
        }
        assert_eq!(fs::read(context.spaces.user_root.join("原始语音.wav")).unwrap(), original);
        assert_eq!(fs::read(sample).unwrap(), original);
        context.shutdown().await.unwrap();
    });
}
