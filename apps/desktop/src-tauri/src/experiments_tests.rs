use super::*;

fn fixture(workspace: &Path) -> ExperimentRecord {
    let id = "ex1000-1";
    ExperimentRecord {
        schema_version: 1, id: id.into(), workspace: workspace.to_string_lossy().into_owned(), created_at: 1000,
        input: ExperimentInput { node_id: "in".into(), original_path: "source.wav".into(),
            snapshot_path: format!(".audio-experiments/{id}/input.wav") },
        output_node_id: "out".into(),
        spec: ExperimentSpec { goal: "compare".into(), base: json!({"mode":"offline","options":{},"graph":{
            "schema_version":1,"nodes":[
                {"id":"in","type":"wav_input","parameters":{"path":format!(".audio-experiments/{id}/input.wav")}},
                {"id":"gain","type":"gain","parameters":{"gain_db":0.0}},
                {"id":"out","type":"wav_output","parameters":{"path":"old.wav"}}
            ],"connections":[]}}),
            parameters: vec![ExperimentParameter { node_id: "gain".into(), parameter_id: "gain_db".into(),
                minimum: -12.0, maximum: 3.0, integer_only: false }] },
        rounds: vec![],
    }
}

fn workspace() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!("audioprocess-experiments-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir(&path).unwrap();
    fs::canonicalize(path).unwrap()
}

fn prepare(workspace: &Path, record: &ExperimentRecord) {
    let folder = root(workspace, true).unwrap().join(&record.id);
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("input.wav"), b"RIFF").unwrap();
    fs::write(folder.join("input.sha256"), digest_file(&folder.join("input.wav")).unwrap()).unwrap();
    write_record(&folder.join("record.json"), record, true).unwrap();
}

fn candidate(record: &ExperimentRecord, round: usize, number: usize, value: f64) -> ExperimentCandidate {
    ExperimentCandidate { id: format!("c{number}"), label: format!("choice {number}"), values: vec![value],
        state: "planned".into(), output_path: format!(".audio-experiments/{}/r{round}-c{number}.wav", record.id),
        task_id: None, result: None, errors: None, feedback: None }
}

#[test]
fn storage_rejects_core_edits_and_retains_prior_bytes() {
    let space = workspace();
    let store = ExperimentStore::default();
    let original = fixture(&space);
    prepare(&space, &original);
    let path = record_path(&space, &original.id).unwrap();
    let before = fs::read(&path).unwrap();
    let mut modified = original.clone();
    modified.spec.goal = "replaced".into();
    assert!(store.save(&space, modified).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    let mut modified = original.clone();
    modified.input.snapshot_path = "elsewhere.wav".into();
    assert!(store.save(&space, modified).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    fs::remove_dir_all(space).unwrap();
}

#[test]
fn hostile_ids_and_candidate_paths_are_rejected() {
    let space = workspace();
    let mut record = fixture(&space);
    record.id = "../outside".into();
    assert!(validate_record(&record, &space).is_err());
    let mut record = fixture(&space);
    record.rounds.push(ExperimentRound { id: "r1".into(), candidates: vec![candidate(&record, 1, 1, 0.0), candidate(&record, 1, 2, 1.0)] });
    assert!(validate_record(&record, &space).is_ok());
    record.rounds[0].candidates[0].output_path = "../escape.wav".into();
    assert!(validate_record(&record, &space).is_err());
    fs::remove_dir_all(space).unwrap();
}

#[test]
fn candidate_values_respect_bounds_and_integer_rule() {
    let mut parameters = vec![ExperimentParameter { node_id: "n".into(), parameter_id: "p".into(), minimum: -2.0,
        maximum: 2.0, integer_only: true }];
    assert!(validate_values(&parameters, &[2.0]).is_ok());
    assert!(validate_values(&parameters, &[2.5]).is_err());
    assert!(validate_values(&parameters, &[f64::NAN]).is_err());
    parameters[0].integer_only = false;
    assert!(validate_values(&parameters, &[1.5]).is_ok());
}

#[test]
fn load_persists_interrupted_state() {
    let space = workspace();
    let store = ExperimentStore::default();
    let mut record = fixture(&space);
    let mut first = candidate(&record, 1, 1, 0.0);
    first.state = "running".into();
    let mut second = candidate(&record, 1, 2, 1.0);
    second.state = "starting".into();
    record.rounds.push(ExperimentRound { id: "r1".into(), candidates: vec![first, second] });
    prepare(&space, &record);
    let loaded = store.load(&space, &record.id).unwrap();
    assert!(loaded.rounds[0].candidates.iter().all(|c| c.state == "interrupted"));
    let persisted = read_record(&record_path(&space, &record.id).unwrap(), &space).unwrap();
    assert!(persisted.rounds[0].candidates.iter().all(|c| c.state == "interrupted"));
    fs::remove_dir_all(space).unwrap();
}

#[test]
fn listing_does_not_interrupt_running_candidates() {
    let space = workspace();
    let store = ExperimentStore::default();
    let mut record = fixture(&space);
    let mut first = candidate(&record, 1, 1, 0.0);
    first.state = "running".into();
    first.task_id = Some("task-1".into());
    record.rounds.push(ExperimentRound { id: "r1".into(), candidates: vec![first, candidate(&record, 1, 2, 1.0)] });
    prepare(&space, &record);
    let path = record_path(&space, &record.id).unwrap();
    let before = fs::read(&path).unwrap();
    assert_eq!(store.list(&space).unwrap().len(), 1);
    assert_eq!(fs::read(path).unwrap(), before);
    fs::remove_dir_all(space).unwrap();
}

#[test]
fn source_refuses_parent_escape() {
    let space = workspace();
    let outside = space.parent().unwrap().join(format!("outside-{}.wav", std::process::id()));
    fs::write(&outside, b"RIFF").unwrap();
    assert!(existing_source(&space, outside.to_str().unwrap()).is_err());
    fs::remove_file(outside).unwrap();
    fs::remove_dir_all(space).unwrap();
}

#[test]
fn record_roundtrip_and_snapshot_tamper_detection() {
    let space = workspace();
    let original = fixture(&space);
    let encoded = serde_json::to_value(&original).unwrap();
    assert!(encoded.get("goal").is_some());
    assert!(encoded.get("spec").is_none());
    let mut unknown = encoded.clone();
    unknown["apiKey"] = json!("must not be stored");
    assert!(serde_json::from_value::<ExperimentRecord>(unknown).is_err());
    let decoded: ExperimentRecord = serde_json::from_value(encoded).unwrap();
    assert!(decoded.spec == original.spec);
    prepare(&space, &original);
    assert!(verify_snapshot(&space, &original).is_ok());
    let manifest = space.join(".audio-experiments").join(&original.id).join("input.sha256");
    fs::write(&manifest, "a".repeat(1024)).unwrap();
    assert!(verify_snapshot(&space, &original).is_err());
    fs::write(&manifest, digest_file(&space.join(&original.input.snapshot_path)).unwrap()).unwrap();
    fs::write(space.join(&original.input.snapshot_path), b"changed").unwrap();
    assert!(verify_snapshot(&space, &original).is_err());
    fs::remove_dir_all(space).unwrap();
}

#[test]
fn succeeded_candidate_requires_result() {
    let space = workspace();
    let mut record = fixture(&space);
    let mut first = candidate(&record, 1, 1, 0.0);
    first.state = "succeeded".into();
    first.task_id = Some("task-1".into());
    record.rounds.push(ExperimentRound { id: "r1".into(), candidates: vec![first, candidate(&record, 1, 2, 1.0)] });
    assert!(validate_record(&record, &space).is_err());
    fs::remove_dir_all(space).unwrap();
}

#[cfg(windows)]
fn control_executable() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for candidate in [root.join("build/Debug/control-cli.exe"), root.join("build/Release/control-cli.exe")] {
        if candidate.is_file() { return fs::canonicalize(candidate).unwrap(); }
    }
    panic!("Build the C++ control-cli target before running the experiment integration test");
}

#[cfg(windows)]
fn tiny_wav() -> Vec<u8> {
    let samples = [0_i16; 64];
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36_u32 + samples.len() as u32 * 2).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&16000_u32.to_le_bytes());
    wav.extend_from_slice(&32000_u32.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(samples.len() as u32 * 2).to_le_bytes());
    for sample in samples { wav.extend_from_slice(&sample.to_le_bytes()); }
    wav
}

#[cfg(windows)]
#[test]
fn local_sidecar_create_run_save_and_reload() {
    use crate::backend::{BackendManager, Connection, DisconnectCallback};
    use std::time::{Duration, Instant};
    let space = workspace();
    fs::write(space.join("source.wav"), tiny_wav()).unwrap();
    let callback: DisconnectCallback = Arc::new(|_| {});
    let connection = Connection::spawn(&control_executable(), space.clone(), "experiment-test".into(),
        false, false, callback).unwrap();
    let backend = BackendManager::with_test_connection(connection);
    let store = ExperimentStore::default();
    let spec = ExperimentSpec { goal: "Compare gain levels".into(), base: json!({"mode":"offline","options":{},"graph":{
        "schema_version":1,"nodes":[
            {"id":"input","type":"wav_input","parameters":{"path":"source.wav"}},
            {"id":"gain","type":"gain","parameters":{"gain_db":0.0}},
            {"id":"output","type":"wav_output","parameters":{"path":"baseline.wav"}}
        ],"connections":[
            {"from":{"node":"input","port":"audio"},"to":{"node":"gain","port":"audio"}},
            {"from":{"node":"gain","port":"audio"},"to":{"node":"output","port":"audio"}}
        ],"exports":[{"name":"file","node":"output","port":"path"}]}}),
        parameters: vec![ExperimentParameter { node_id: "gain".into(), parameter_id: "gain_db".into(),
            minimum: -6.0, maximum: 0.0, integer_only: false }] };
    let mut record = store.create(&backend, "experiment-test", spec).unwrap();
    assert!(space.join(&record.input.snapshot_path).is_file());
    let first = candidate(&record, 1, 1, -6.0);
    let second = candidate(&record, 1, 2, 0.0);
    record.rounds.push(ExperimentRound { id: "r1".into(), candidates: vec![first, second] });
    store.save_checked(&backend, "experiment-test", record.clone()).unwrap();
    record.rounds[0].candidates[0].state = "starting".into();
    store.save_checked(&backend, "experiment-test", record.clone()).unwrap();
    let mut graph = record.spec.base["graph"].clone();
    graph["nodes"][1]["parameters"]["gain_db"] = json!(-6.0);
    graph["nodes"][2]["parameters"]["path"] = json!(record.rounds[0].candidates[0].output_path);
    assert_eq!(backend.request("experiment-test", json!({"op":"graph.validate","mode":"offline","graph":graph})).unwrap()["success"], true);
    let started = backend.request("experiment-test", json!({"op":"tasks.start","mode":"offline","graph":graph})).unwrap();
    assert_eq!(started["success"], true);
    let task_id = started["data"]["task_id"].as_str().unwrap().to_owned();
    record.rounds[0].candidates[0].task_id = Some(task_id.clone());
    record.rounds[0].candidates[0].state = "running".into();
    store.save_checked(&backend, "experiment-test", record.clone()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let status = backend.request("experiment-test", json!({"op":"tasks.status","task_id":task_id})).unwrap();
        if status["data"]["state"] == "succeeded" { break; }
        assert!(Instant::now() < deadline, "experiment task did not succeed: {status}");
        std::thread::sleep(Duration::from_millis(2));
    }
    let result = backend.request("experiment-test", json!({"op":"tasks.result","task_id":task_id})).unwrap();
    assert_eq!(result["success"], true);
    record.rounds[0].candidates[0].result = result.pointer("/data/result").cloned();
    record.rounds[0].candidates[0].state = "succeeded".into();
    store.save_checked(&backend, "experiment-test", record.clone()).unwrap();
    backend.request("experiment-test", json!({"op":"tasks.release","task_id":task_id})).unwrap();
    let reloaded = store.load_checked(&backend, "experiment-test", &record.id).unwrap();
    assert_eq!(reloaded.rounds[0].candidates[0].state, "succeeded");
    assert!(space.join(&reloaded.rounds[0].candidates[0].output_path).is_file());
    fs::write(space.join(&record.input.snapshot_path), b"modified").unwrap();
    assert!(store.load_checked(&backend, "experiment-test", &record.id).is_err());
    backend.shutdown().unwrap();
    fs::remove_dir_all(space).unwrap();
}
