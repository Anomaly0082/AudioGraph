use super::*;
use serde_json::Value;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!("audioprocess-plugin-snapshot-{}-{}",
            std::process::id(),SERIAL.fetch_add(1,Ordering::Relaxed)));
        fs::create_dir(&root).unwrap();
        Self(fs::canonicalize(root).unwrap())
    }
    fn packages(&self) -> PathBuf { self.0.join("config/plugins") }
    fn data(&self) -> PathBuf { self.0.join("data") }
    fn package(&self,name: &str,bytes: &[u8]) -> PathBuf {
        let path = self.packages().join(name);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("manifest.json"),bytes).unwrap();
        path
    }
    fn workspace(&self,name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir(&path).unwrap();
        fs::canonicalize(path).unwrap()
    }
}
impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

#[test]
fn empty_installation_creates_only_its_directory_and_does_not_pick_up_later_packages() {
    let fixture = Fixture::new();
    let captured = PluginSnapshot::capture(&fixture.packages(),&fixture.data()).unwrap();
    assert_eq!(captured.plugin_directory().unwrap(),fixture.packages());
    assert!(fixture.packages().is_dir());
    assert_eq!(fs::read_dir(fixture.packages()).unwrap().count(),0);
    assert!(captured.stored.is_none());
    assert!(!fixture.data().exists());
    fixture.package("added-after-start",b"{}");
    assert!(captured.launch_for(&fixture.workspace("workspace")).unwrap().is_none());
    assert!(!fixture.data().exists());
    let next_start = PluginSnapshot::capture(&fixture.packages(),&fixture.data()).unwrap();
    assert!(next_start.stored.is_some());
}

#[test]
fn installation_directory_creation_is_idempotent_and_preserves_existing_files() {
    let fixture = Fixture::new();
    let first = PluginSnapshot::capture(&fixture.packages(),&fixture.data()).unwrap();
    fs::write(fixture.packages().join("note.txt"),b"keep this file").unwrap();
    let again = PluginSnapshot::capture(&fixture.packages(),&fixture.data()).unwrap();
    assert_eq!(first.plugin_directory(),again.plugin_directory());
    assert_eq!(fs::read(fixture.packages().join("note.txt")).unwrap(),b"keep this file");
    assert!(again.stored.is_none());
    assert!(!fixture.data().exists());
}

#[test]
fn installation_directory_never_replaces_files_and_validates_storage_before_creating() {
    let fixture = Fixture::new();
    assert!(PluginSnapshot::capture(&fixture.packages(),Path::new("relative/data")).is_err());
    assert!(!fixture.packages().exists());
    fs::write(fixture.0.join("config"),b"parent file").unwrap();
    assert!(PluginSnapshot::capture(&fixture.packages(),&fixture.data()).is_err());
    assert_eq!(fs::read(fixture.0.join("config")).unwrap(),b"parent file");

    let other = Fixture::new();
    fs::create_dir(other.0.join("config")).unwrap();
    fs::write(other.packages(),b"directory name occupied by file").unwrap();
    assert!(PluginSnapshot::capture(&other.packages(),&other.data()).is_err());
    assert_eq!(fs::read(other.packages()).unwrap(),b"directory name occupied by file");
    assert!(!other.data().exists());
}

#[test]
fn package_selection_is_sorted_bounded_and_hashes_exact_manifest_bytes() {
    let fixture = Fixture::new();
    fixture.package("z",b"not JSON: Rust only hashes; C++ reports invalid packages");
    let a = fixture.package("a",b"{\r\n\"experimental\":true\r\n}");
    let captured = PluginSnapshot::capture(&fixture.packages(),&fixture.data()).unwrap();
    let stored = captured.stored.as_ref().unwrap();
    let bytes = fs::read(&stored.path).unwrap();
    assert_eq!(digest(&bytes),stored.sha256);
    let document: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(document.as_object().unwrap().len(),2);
    assert_eq!(document["schema_version"],1);
    assert_eq!(document["packages"].as_array().unwrap().len(),2);
    assert_eq!(document["packages"][0].as_object().unwrap().len(),2);
    assert_eq!(document["packages"][0]["root"],a.to_str().unwrap());
    assert_eq!(document["packages"][0]["manifest_sha256"],digest(&fs::read(a.join("manifest.json")).unwrap()));
    assert!(!fixture.data().join("plugin-data").exists());
}

#[test]
fn launches_reuse_snapshot_without_rescanning_and_isolate_workspace_data() {
    let fixture = Fixture::new();
    fixture.package("first",b"{}");
    let captured = PluginSnapshot::capture(&fixture.packages(),&fixture.data()).unwrap();
    let workspace = fixture.workspace("user");
    let first = captured.launch_for(&workspace).unwrap().unwrap();
    fixture.package("later",b"{}");
    let again = captured.launch_for(&workspace).unwrap().unwrap();
    let ai = captured.launch_for(&fixture.workspace("ai")).unwrap().unwrap();
    assert_eq!(first.snapshot_path,again.snapshot_path);
    assert_eq!(first.snapshot_sha256,again.snapshot_sha256);
    assert_eq!(first.data_root,again.data_root);
    assert_ne!(first.data_root,ai.data_root);
    assert_eq!(first.snapshot_sha256,ai.snapshot_sha256);
    assert!(first.data_root.is_dir());
    let document: Value = serde_json::from_slice(&fs::read(&first.snapshot_path).unwrap()).unwrap();
    assert_eq!(document["packages"].as_array().unwrap().len(),1);
    let next_start = PluginSnapshot::capture(&fixture.packages(),&fixture.data()).unwrap();
    assert_ne!(next_start.stored.as_ref().unwrap().path,first.snapshot_path);
    assert_eq!(serde_json::from_slice::<Value>(&fs::read(&next_start.stored.as_ref().unwrap().path).unwrap()).unwrap()["packages"].as_array().unwrap().len(),2);
}

#[test]
fn tampered_or_hardlinked_metadata_is_rejected_without_overwriting() {
    let fixture = Fixture::new();
    let package = fixture.package("first",b"{}");
    let captured = PluginSnapshot::capture(&fixture.packages(),&fixture.data()).unwrap();
    let stored = captured.stored.as_ref().unwrap();
    fs::write(&stored.path,b"tampered").unwrap();
    assert!(captured.launch_for(&fixture.workspace("user")).is_err());
    assert_eq!(fs::read(&stored.path).unwrap(),b"tampered");
    fs::hard_link(package.join("manifest.json"),fixture.0.join("linked-manifest.json")).unwrap();
    assert!(PluginSnapshot::capture(&fixture.packages(),&fixture.data()).is_err());
}

#[test]
fn malformed_storage_paths_and_package_limits_do_not_silently_select_empty_plugins() {
    let fixture = Fixture::new();
    assert!(PluginSnapshot::capture(Path::new("relative/plugins"),&fixture.data()).is_err());
    // Windows PathBuf::push normalizes '..' for verbatim prefixes. Construct an
    // ordinary raw input string so this tests a caller's actual traversal path.
    let raw = fixture.0.to_string_lossy();
    let ordinary = raw.strip_prefix(r"\\?\").unwrap_or(raw.as_ref());
    let traversal = PathBuf::from(format!("{ordinary}{separator}..{separator}plugins",separator=std::path::MAIN_SEPARATOR));
    assert!(PluginSnapshot::capture(&traversal,&fixture.data()).is_err());
    assert!(PluginSnapshot::capture(fixture.0.ancestors().last().unwrap(),&fixture.data()).is_err());
    fixture.package("big",&vec![b'x';MANIFEST_LIMIT+1]);
    assert!(PluginSnapshot::capture(&fixture.packages(),&fixture.data()).is_err());
    fs::write(fixture.packages().join("big/manifest.json"),b"{}").unwrap();
    for index in 0..PACKAGE_LIMIT { fixture.package(&format!("package-{index}"),b"{}"); }
    assert!(PluginSnapshot::capture(&fixture.packages(),&fixture.data()).is_err());
    assert!(!fixture.data().exists());
}

#[cfg(any(windows,unix))]
#[test]
fn linked_package_directory_cannot_expand_the_fixed_installation_scope() {
    let fixture = Fixture::new();
    let outside = fixture.0.join("outside-package");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("manifest.json"),b"{}").unwrap();
    fs::create_dir_all(fixture.packages()).unwrap();
    let linked = fixture.packages().join("linked");
    #[cfg(windows)]
    let created = std::os::windows::fs::symlink_dir(&outside,&linked);
    #[cfg(unix)]
    let created = std::os::unix::fs::symlink(&outside,&linked);
    if let Err(error) = created {
        #[cfg(windows)]
        if error.kind() == std::io::ErrorKind::PermissionDenied || error.raw_os_error() == Some(1314) {
            eprintln!("Skipping Windows symlink fixture: this process lacks symlink privilege (error {error})");
            return;
        }
        panic!("Cannot create test link: {error}");
    }
    assert!(PluginSnapshot::capture(&fixture.packages(),&fixture.data()).is_err());
    assert!(!fixture.data().exists());
}

#[cfg(windows)]
#[test]
fn captured_real_sample_package_is_available_to_user_and_related_ai_sidecars_without_rescanning() {
    use crate::backend::{BackendManager,Connection,DisconnectCallback};
    use std::sync::Arc;
    let fixture = Fixture::new();
    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let package = ["Debug","Release"].into_iter().map(|configuration|
        project.join("build/plugin-sdk-standalone/plugin-build/package").join(configuration))
        .find(|path| path.join("ag_example_plugin.dll").is_file() && path.join("manifest.json").is_file())
        .expect("Build the standalone installed-SDK example package before integration tests");
    let installed = fixture.packages().join("example");
    fs::create_dir_all(&installed).unwrap();
    for name in ["ag_example_plugin.dll","manifest.json","nodes.json"] {
        fs::copy(package.join(name),installed.join(name)).unwrap();
    }
    let executable = [project.join("build/Debug/control-cli.exe"),project.join("build/Release/control-cli.exe")]
        .into_iter().find(|path| path.is_file()).expect("Build the plugin-aware fixed control-cli before integration tests");
    let snapshot = Arc::new(PluginSnapshot::capture(&fixture.packages(),&fixture.data()).unwrap());
    let shared = BackendManager::default();
    shared.configure_plugin_snapshot(Ok(snapshot.clone())).unwrap();
    let ai = shared.related_manager();
    assert!(Arc::ptr_eq(&snapshot,&ai.plugin_snapshot().unwrap()));
    let user_workspace = fixture.workspace("user-audio");
    let ai_workspace = fixture.workspace("ai-audio");
    let user_launch = snapshot.launch_for(&user_workspace).unwrap().unwrap();
    let ai_launch = ai.plugin_snapshot().unwrap().launch_for(&ai_workspace).unwrap().unwrap();
    assert_eq!(user_launch.snapshot_path,ai_launch.snapshot_path);
    assert_eq!(user_launch.snapshot_sha256,ai_launch.snapshot_sha256);
    assert_ne!(user_launch.data_root,ai_launch.data_root);
    // A directory added after capture must not become a package in either session.
    fixture.package("invalid-added-after-start",b"{}");
    let callback: DisconnectCallback = Arc::new(|_| {});
    for (workspace,session,selected) in [
        (user_workspace,"snapshot-user",shared.plugin_snapshot().unwrap()),
        (ai_workspace,"snapshot-ai",ai.plugin_snapshot().unwrap()),
    ] {
        let mut wav = vec![
            b'R',b'I',b'F',b'F',44,0,0,0,b'W',b'A',b'V',b'E',b'f',b'm',b't',b' ',
            16,0,0,0,1,0,1,0,0x44,0xac,0,0,0x88,0x58,1,0,2,0,16,0,
            b'd',b'a',b't',b'a',8,0,0,0,
        ];
        for sample in [12000_i16,-12000,4000,-4000] { wav.extend_from_slice(&sample.to_le_bytes()); }
        fs::write(workspace.join("input.wav"),wav).unwrap();
        let sidecar = Connection::spawn_with_plugins(&executable,workspace.clone(),session.into(),
            false,false,callback.clone(),selected).unwrap();
        let capabilities = sidecar.request(serde_json::json!({"op":"capabilities"})).unwrap();
        assert_eq!(capabilities["success"],true,"{capabilities}");
        assert!(capabilities["data"]["plugins"]["errors"].as_array().unwrap().is_empty(),"{capabilities}");
        let catalog = sidecar.request(serde_json::json!({"op":"nodes.list"})).unwrap();
        assert_eq!(catalog["success"],true,"{catalog}");
        let nodes = catalog["data"]["nodes"].as_array().unwrap();
        for id in ["org.audiograph.example.gain_v1","org.audiograph.example.mock_asr_v1"] {
            assert!(nodes.iter().any(|node| node["typeId"] == id),"{catalog}");
        }
        let graph = serde_json::json!({"schema_version":1,"nodes":[
            {"id":"source","type":"wav_input","parameters":{"path":"input.wav"}},
            {"id":"gain","type":"org.audiograph.example.gain_v1","parameters":{"gain_db":-6.020599913279624}},
            {"id":"output","type":"wav_output","parameters":{"path":"result.wav"}}
        ],"connections":[
            {"from":{"node":"source","port":"audio"},"to":{"node":"gain","port":"audio"}},
            {"from":{"node":"gain","port":"audio"},"to":{"node":"output","port":"audio"}}
        ],"exports":[{"name":"file","node":"output","port":"path"}]});
        let started = sidecar.request(serde_json::json!({"op":"tasks.start","mode":"offline","graph":graph})).unwrap();
        assert_eq!(started["success"],true,"{started}");
        let task = started["data"]["task_id"].as_str().unwrap();
        let deadline = Instant::now()+Duration::from_secs(5);
        loop {
            let status = sidecar.request(serde_json::json!({"op":"tasks.status","task_id":task})).unwrap();
            assert_eq!(status["success"],true,"{status}");
            match status["data"]["state"].as_str() {
                Some("succeeded") => break,
                Some("queued" | "running" | "cancelling") => {},
                _ => panic!("Plugin gain task failed: {status}"),
            }
            assert!(Instant::now() < deadline,"Plugin gain task exceeded the test deadline");
            std::thread::sleep(Duration::from_millis(2));
        }
        let result = sidecar.request(serde_json::json!({"op":"tasks.result","task_id":task})).unwrap();
        assert_eq!(result["success"],true,"{result}");
        assert_eq!(result["data"]["state"],"succeeded","{result}");
        sidecar.request(serde_json::json!({"op":"tasks.release","task_id":task})).unwrap();
        let rendered = fs::read(workspace.join("result.wav")).unwrap();
        assert_eq!(&rendered[..4],b"RIFF");
        assert_eq!(&rendered[8..12],b"WAVE");
        let mut offset = 12usize;
        let mut samples = None;
        while offset+8 <= rendered.len() {
            let size = u32::from_le_bytes(rendered[offset+4..offset+8].try_into().unwrap()) as usize;
            let start = offset+8;
            assert!(start+size <= rendered.len());
            if &rendered[offset..offset+4] == b"data" {
                samples = Some(rendered[start..start+size].chunks_exact(2)
                    .map(|bytes| i16::from_le_bytes(bytes.try_into().unwrap())).collect::<Vec<_>>());
                break;
            }
            offset = start+size+size%2;
        }
        let samples = samples.expect("Rendered PCM16 WAV includes a data chunk");
        assert_eq!(samples.len(),4);
        for (actual,expected) in samples.into_iter().zip([6000_i16,-6000,2000,-2000]) {
            assert!((i32::from(actual)-i32::from(expected)).abs() <= 2,"Plugin gain sample {actual} differs from half amplitude {expected}");
        }
        sidecar.shutdown("Plugin startup snapshot integration complete").unwrap();
    }
}
