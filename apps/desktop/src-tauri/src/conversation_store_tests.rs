use super::*;

struct Fixture { root: PathBuf, app: PathBuf, spaces: ToolWorkspaces }
impl Fixture {
    fn new() -> Self {
        let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("audioprocess_conversations_{}_{nonce}_{serial}", std::process::id()));
        let user = root.join("user");
        let app = root.join("app-data");
        fs::create_dir_all(&user).unwrap();
        fs::create_dir(&app).unwrap();
        let spaces = ToolWorkspaces::new(&user, &app).unwrap();
        Self { root, app, spaces }
    }
    fn create(&self, store: &ConversationStore, mode: &str) -> ConversationDetail {
        store.create(&self.spaces, &self.app, mode, None).unwrap()
    }
    fn begin(&self, store: &Arc<ConversationStore>, id: &str, request: &str) -> ConversationTurnGuard {
        store.begin_turn(&self.spaces, &self.app, id, "graph", request, "Process this audio", None).unwrap()
    }
    fn load(&self, store: &ConversationStore, id: &str) -> ConversationDetail {
        store.load(&self.spaces, &self.app, id, None, None).unwrap()
    }
}
impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.root); } }

fn reply(id: &str) -> Value {
    json!({"request_id":id,"state":"completed","text":"Done","events":[{"kind":"assistant","text":"Done"}],"model_calls":1,"tool_calls":0})
}

#[test]
fn read_only_listing_does_not_create_storage_or_change_disk() {
    let f = Fixture::new();
    let store = Arc::new(ConversationStore::default());
    let dir = storage_dir(&f.spaces, &f.app, false).unwrap();
    assert!(!dir.exists());
    assert!(store.list(&f.spaces, &f.app).unwrap().records.is_empty());
    assert!(!dir.exists());
    let detail = f.create(&store, "graph");
    assert!(!dir.starts_with(&f.spaces.user_root));
    assert!(!dir.starts_with(&f.spaces.ai_root));
    let path = record_path(&dir, &detail.id).unwrap();
    let before = fs::read(&path).unwrap();
    f.load(&store, &detail.id);
    store.list(&f.spaces, &f.app).unwrap();
    assert_eq!(before, fs::read(path).unwrap());
}

#[test]
fn dropped_or_restarted_turn_is_interrupted_without_replay_or_disk_write() {
    let f = Fixture::new();
    let store = Arc::new(ConversationStore::default());
    let detail = f.create(&store, "graph");
    let mut guard = f.begin(&store, &detail.id, "req-1");
    guard.checkpoint(json!([]), vec![], vec![ToolIntent::new("call-1".into(), "file_write".into(), Some(json!({"path":"output.txt","content":"never replay"})))]).unwrap();
    assert_eq!(f.load(&store, &detail.id).turns[0].state, "running");
    let path = record_path(&storage_dir(&f.spaces, &f.app, false).unwrap(), &detail.id).unwrap();
    let before = fs::read(&path).unwrap();
    let restarted = ConversationStore::default();
    let recovered = f.load(&restarted, &detail.id);
    assert_eq!(recovered.turns[0].state, "interrupted");
    assert_eq!(recovered.turns[0].pending_tools[0].name, "file_write");
    assert_eq!(before, fs::read(&path).unwrap());
    assert!(!f.spaces.user_root.join("output.txt").exists());
    drop(guard);
    assert_eq!(f.load(&store, &detail.id).turns[0].state, "interrupted");
    assert_eq!(before, fs::read(path).unwrap());
}

#[test]
fn duplicate_request_ids_and_mode_changes_are_rejected_persistently() {
    let f = Fixture::new();
    let store = Arc::new(ConversationStore::default());
    let detail = f.create(&store, "graph");
    let mut guard = f.begin(&store, &detail.id, "req-1");
    assert!(store.begin_turn(&f.spaces, &f.app, &detail.id, "graph", "req-2", "Prompt", None).is_err());
    guard.finish(reply("req-1"), vec![]).unwrap();
    assert!(guard.finish(reply("req-1"), vec![]).is_err());
    let restarted = Arc::new(ConversationStore::default());
    assert!(restarted.begin_turn(&f.spaces, &f.app, &detail.id, "graph", "req-1", "Prompt", None).is_err());
    assert!(restarted.begin_turn(&f.spaces, &f.app, &detail.id, "workflow", "req-2", "Prompt", None).is_err());
    let guard = f.begin(&restarted, &detail.id, "req-2");
    assert_eq!(guard.prior_turns.len(), 1);
    assert_eq!(guard.prior_turns[0].reply.as_ref().unwrap().text, "Done");
}

#[test]
fn visible_reply_allowlist_and_nested_private_fields_are_enforced() {
    let f = Fixture::new();
    let store = Arc::new(ConversationStore::default());
    let detail = f.create(&store, "graph");
    let prompt = "User text mentions APIkey=mine and reasoning_content literally";
    let mut guard = store.begin_turn(&f.spaces, &f.app, &detail.id, "graph", "req-1", prompt,
        Some(json!({"nodes":[],"api_key":"graph-secret"}))).unwrap();
    let mut visible = reply("req-1");
    visible["config"] = json!({"api_key":"config-secret"});
    visible["reasoning_content"] = json!("provider-secret");
    visible["events"] = json!([{"kind":"tool","tool":"graph_run","arguments":{"api_key":"argument-secret","node":"kept"},
        "result":{"data":{"authorization":"auth-secret","reasoning_content":"nested-secret","ok":true}},"success":true,
        "reasoning_content":"event-secret"}]);
    guard.finish(visible, vec![]).unwrap();
    let loaded = f.load(&store, &detail.id);
    assert_eq!(loaded.turns[0].prompt, prompt);
    let path = record_path(&storage_dir(&f.spaces, &f.app, false).unwrap(), &detail.id).unwrap();
    let raw = fs::read_to_string(path).unwrap();
    for forbidden in ["graph-secret","config-secret","provider-secret","argument-secret","auth-secret","nested-secret","event-secret"] {
        assert!(!raw.contains(forbidden), "Unexpected private field: {forbidden}");
    }
    assert!(raw.contains("User text mentions APIkey=mine"));
    assert_eq!(loaded.turns[0].reply.as_ref().unwrap().events[0].arguments.as_ref().unwrap()["node"], "kept");
}

#[test]
fn omitted_tool_payloads_keep_metadata_and_all_run_ids() {
    let f = Fixture::new();
    let store = Arc::new(ConversationStore::default());
    let detail = f.create(&store, "graph");
    let mut guard = f.begin(&store, &detail.id, "req-1");
    let run_id = "a".repeat(64);
    let events = json!([{"kind":"tool","tool":"graph_run","success":true,
        "result":{"data":{"run_id":run_id,"payload":"x".repeat(2*1024*1024)}}}]);
    guard.checkpoint(events.clone(), vec![], vec![ToolIntent::new("future".into(), "file_write".into(), Some(json!({"content":"y".repeat(65536)})))]).unwrap();
    let pending = f.load(&store, &detail.id);
    assert!(pending.turns[0].events[0].omitted_bytes.is_some());
    assert!(pending.turns[0].pending_tools[0].omitted_bytes.is_some());
    assert_eq!(pending.turns[0].run_ids, vec![run_id.clone()]);
    assert_eq!(pending.turns[0].events[0].result.as_ref().unwrap()["omitted"], true);
    let mut final_reply = reply("req-1");
    final_reply["events"] = events;
    assert!(guard.finish(final_reply.clone(), vec![]).is_err());
    guard.checkpoint(json!([]), vec![], vec![]).unwrap();
    guard.finish(final_reply, vec![]).unwrap();
    let saved = f.load(&store, &detail.id);
    assert_eq!(saved.turns[0].run_ids, vec![run_id]);
    assert!(saved.turns[0].reply.as_ref().unwrap().events[0].omitted_bytes.is_some());
}

#[test]
fn pages_are_recent_twenty_then_earlier_and_history_is_bounded() {
    let f = Fixture::new();
    let store = Arc::new(ConversationStore::default());
    let detail = f.create(&store, "graph");
    for i in 0..25 {
        let id = format!("request-{i}");
        let mut guard = f.begin(&store, &detail.id, &id);
        assert_eq!(guard.prior_turns.len(), i.min(3));
        guard.finish(reply(&id), vec![]).unwrap();
    }
    let recent = f.load(&store, &detail.id);
    assert_eq!(recent.turns.len(), 20);
    assert_eq!(recent.turns[0].id, "request-5");
    assert_eq!(recent.before, Some(5));
    assert!(recent.has_more);
    let earlier = store.load(&f.spaces, &f.app, &detail.id, recent.before, Some(20)).unwrap();
    assert_eq!(earlier.turns.len(), 5);
    assert_eq!(earlier.turns[0].id, "request-0");
    assert!(!earlier.has_more);
    assert!(store.load(&f.spaces, &f.app, &detail.id, Some(26), None).is_err());
}

#[test]
fn oversized_user_prompt_or_context_rejected_without_recording_a_turn() {
    let f = Fixture::new();
    let store = Arc::new(ConversationStore::default());
    let detail = f.create(&store, "graph");
    assert!(store.begin_turn(&f.spaces, &f.app, &detail.id, "graph", "long", &"x".repeat(MAX_PROMPT+1), None).is_err());
    assert!(store.begin_turn(&f.spaces, &f.app, &detail.id, "graph", "context", "Prompt", Some(json!({"body":"x".repeat(MAX_CONTEXT)}))).is_err());
    assert!(f.load(&store, &detail.id).turns.is_empty());
}

#[test]
fn hundred_turn_limit_preserves_all_history_and_refuses_new_turn() {
    let f = Fixture::new();
    let store = Arc::new(ConversationStore::default());
    let detail = f.create(&store, "graph");
    for i in 0..MAX_TURNS {
        let id = format!("request-{i}");
        let mut guard = f.begin(&store, &detail.id, &id);
        guard.finish(reply(&id), vec![]).unwrap();
    }
    assert!(store.begin_turn(&f.spaces, &f.app, &detail.id, "graph", "overflow", "Prompt", None).err().unwrap().contains("full"));
    assert_eq!(f.load(&store, &detail.id).turn_count, MAX_TURNS);
}

#[test]
fn byte_capacity_refuses_begin_without_trimming_prior_turns() {
    let f = Fixture::new();
    let store = Arc::new(ConversationStore::default());
    let detail = f.create(&store, "graph");
    let mut guard = store.begin_turn(&f.spaces, &f.app, &detail.id, "graph", "first", "Prompt",
        Some(json!({"graph":"x".repeat(MAX_CONTEXT - 32)}))).unwrap();
    let mut final_reply = reply("first");
    final_reply["text"] = json!("x".repeat(MAX_TEXT));
    guard.finish(final_reply, vec![]).unwrap();
    let dir = storage_dir(&f.spaces, &f.app, false).unwrap();
    let mut record = read_record(&dir, &detail.id).unwrap();
    let template = record.turns[0].clone();
    record.turns = (0..34).map(|i| {
        let mut turn = template.clone();
        turn.id = format!("prior-{i}");
        turn.reply.as_mut().unwrap().request_id = turn.id.clone();
        turn
    }).collect();
    write_record(&dir, &record, true).unwrap();
    let path = record_path(&dir, &detail.id).unwrap();
    let before = fs::read(&path).unwrap();
    let error = store.begin_turn(&f.spaces, &f.app, &detail.id, "graph", "too-full", "Prompt", None).err().unwrap();
    assert!(error.contains("full"));
    assert_eq!(before, fs::read(path).unwrap());
    assert_eq!(f.load(&store, &detail.id).turn_count, 34);
}

#[test]
fn reserved_turn_budget_handles_json_escape_expansion() {
    let f = Fixture::new();
    let store = Arc::new(ConversationStore::default());
    let detail = f.create(&store, "graph");
    let mut guard = f.begin(&store, &detail.id, "escaped");
    let events = Value::Array((0..MAX_EVENTS).map(|_| json!({"kind":"assistant","text":"\u{1}".repeat(EVENT_BYTES)})).collect());
    guard.checkpoint(events.clone(), vec![], vec![]).unwrap();
    let mut result = reply("escaped");
    result["state"] = json!("failed");
    result["text"] = json!("\u{1}".repeat(MAX_TEXT));
    result["events"] = events;
    guard.finish(result, vec![]).unwrap();
    let saved = f.load(&store, &detail.id);
    assert_eq!(saved.turns[0].state, "failed");
    assert!(encoded_size(&saved.turns[0]).unwrap() < TURN_RESERVE);
}

#[test]
fn workspace_isolation_and_invalid_id_paths_are_enforced() {
    let f = Fixture::new();
    let store = Arc::new(ConversationStore::default());
    let detail = f.create(&store, "workflow");
    let other = f.root.join("other-user");
    fs::create_dir(&other).unwrap();
    let spaces = ToolWorkspaces::new(&other, &f.app).unwrap();
    assert!(store.list(&spaces, &f.app).unwrap().records.is_empty());
    assert!(store.load(&spaces, &f.app, &detail.id, None, None).is_err());
    assert!(store.load(&f.spaces, &f.app, "../escape", None, None).is_err());
}

#[test]
fn corrupt_files_warn_without_hiding_valid_conversations() {
    let f = Fixture::new();
    let store = Arc::new(ConversationStore::default());
    let good = f.create(&store, "graph");
    let dir = storage_dir(&f.spaces, &f.app, false).unwrap();
    fs::write(record_path(&dir, &"f".repeat(64)).unwrap(), b"invalid JSON").unwrap();
    let listed = store.list(&f.spaces, &f.app).unwrap();
    assert_eq!(listed.records.len(), 1);
    assert_eq!(listed.records[0].id, good.id);
    assert_eq!(listed.warnings.len(), 1);
}

#[test]
fn hardlinked_records_cannot_be_loaded_or_replaced() {
    let f = Fixture::new();
    let store = Arc::new(ConversationStore::default());
    let detail = f.create(&store, "graph");
    let dir = storage_dir(&f.spaces, &f.app, false).unwrap();
    let path = record_path(&dir, &detail.id).unwrap();
    let before = fs::read(&path).unwrap();
    let alias = f.root.join("alias.json");
    fs::hard_link(&path, &alias).unwrap();
    assert!(store.load(&f.spaces, &f.app, &detail.id, None, None).is_err());
    assert!(store.begin_turn(&f.spaces, &f.app, &detail.id, "graph", "request", "Prompt", None).is_err());
    assert_eq!(before, fs::read(&alias).unwrap());
}

#[cfg(unix)]
#[test]
fn symlinked_record_and_storage_component_are_rejected() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let store = Arc::new(ConversationStore::default());
    let detail = f.create(&store, "graph");
    let dir = storage_dir(&f.spaces, &f.app, false).unwrap();
    let link_id = "b".repeat(64);
    symlink(record_path(&dir, &detail.id).unwrap(), record_path(&dir, &link_id).unwrap()).unwrap();
    assert!(store.load(&f.spaces, &f.app, &link_id, None, None).is_err());
    let other_app = f.root.join("other-app");
    fs::create_dir(&other_app).unwrap();
    symlink(f.app.join("conversations"), other_app.join("conversations")).unwrap();
    assert!(store.list(&f.spaces, &other_app).is_err());
}
