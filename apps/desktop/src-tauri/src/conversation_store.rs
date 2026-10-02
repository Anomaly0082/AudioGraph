//! Visible conversation records only. Reading history never resumes a model or a tool.
use crate::run_records::{check_chain, create_file, ensure_dir, is_hardlinked,
    opened_file_within_workspace, overlaps, replace_file, safe_metadata, temp_file, time_ms, valid_id};
use crate::tool_workspaces::ToolWorkspaces;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, atomic::{AtomicU64, Ordering}};

const RECORD_LIMIT: usize = 8 * 1024 * 1024;
// Includes JSON escape expansion, partial events, a final reply, and its error.
const TURN_RESERVE: usize = 2 * 1024 * 1024;
const MAX_TURNS: usize = 100;
const MAX_PROMPT: usize = 16 * 1024;
const MAX_CONTEXT: usize = 128 * 1024;
const MAX_EVENTS: usize = 256;
const EVENT_BYTES: usize = 48 * 1024;
const EVENTS_BYTES: usize = 192 * 1024;
const MAX_TEXT: usize = 64 * 1024;
const MAX_INTENTS: usize = 40;
const INTENT_BYTES: usize = 2 * 1024;
const MAX_RUN_IDS: usize = 256;
const LIST_SCAN: usize = 10_000;
const LIST_RECORDS: usize = 500;
const LIST_BYTES: u64 = 32 * 1024 * 1024;
static SERIAL: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredEvent {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub omitted_bytes: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredReply {
    pub request_id: String,
    pub state: String,
    pub text: String,
    pub events: Vec<StoredEvent>,
    pub model_calls: usize,
    pub tool_calls: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub omitted_text_bytes: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolIntent {
    pub id: String,
    pub name: String,
    pub arguments: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub omitted_bytes: Option<usize>,
}

impl ToolIntent {
    pub fn new(id: String, name: String, arguments: Option<Value>) -> Self {
        Self { id, name, arguments, omitted_bytes: None }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationTurn {
    pub id: String,
    pub prompt: String,
    pub context_graph: Option<Value>,
    pub started_at_ms: u64,
    pub finished_at_ms: Option<u64>,
    pub state: String,
    pub reply: Option<StoredReply>,
    pub error: Option<String>,
    pub events: Vec<StoredEvent>,
    pub run_ids: Vec<String>,
    pub pending_tools: Vec<ToolIntent>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConversationRecord {
    schema_version: u32,
    id: String,
    mode: String,
    title: String,
    created_at_ms: u64,
    updated_at_ms: u64,
    turns: Vec<ConversationTurn>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConversationSummary {
    pub id: String,
    pub title: String,
    pub mode: String,
    pub updated_at_ms: u64,
    pub turn_count: usize,
    pub last_state: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConversationDetail {
    pub id: String,
    pub mode: String,
    pub title: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub turn_count: usize,
    pub last_state: String,
    pub turns: Vec<ConversationTurn>,
    pub before: Option<usize>,
    pub has_more: bool,
}

#[derive(Serialize)]
pub struct ConversationList {
    pub records: Vec<ConversationSummary>,
    pub warnings: Vec<String>,
    pub truncated: bool,
}

type ActiveKey = (PathBuf, String, String);

/// The same managed store must serve commands and the runtime. The lock serializes
/// file updates with the process-local leases, including duplicate request checks.
#[derive(Default)]
pub struct ConversationStore {
    active: Mutex<HashSet<ActiveKey>>,
}

pub struct ConversationTurnGuard {
    store: Arc<ConversationStore>,
    dir: PathBuf,
    conversation_id: String,
    request_id: String,
    finished: bool,
    pub prior_turns: Vec<ConversationTurn>,
}

fn storage_dir(spaces: &ToolWorkspaces, app_data: &Path, create: bool) -> Result<PathBuf, String> {
    if !app_data.is_absolute() || app_data.components().any(|p| matches!(p, Component::ParentDir | Component::CurDir)) {
        return Err("App data path must be absolute and normalized".into());
    }
    check_chain(app_data)?;
    let app_data = fs::canonicalize(app_data).map_err(|e| format!("Cannot locate app data: {e}"))?;
    check_chain(&app_data)?;
    if overlaps(&app_data, &spaces.user_root) { return Err("Conversations must be outside user workspace".into()); }
    let identity = if cfg!(windows) { spaces.user_root.to_string_lossy().to_lowercase() }
        else { spaces.user_root.to_string_lossy().into_owned() };
    let hash = format!("{:x}", Sha256::digest(identity.as_bytes()));
    let mut dir = app_data;
    for part in ["conversations", "v1", &hash] {
        dir.push(part);
        if create { ensure_dir(&dir)?; } else { check_chain(&dir)?; }
    }
    if overlaps(&dir, &spaces.ai_root) { return Err("Conversations must be outside AI workspace".into()); }
    Ok(dir)
}

fn record_path(dir: &Path, id: &str) -> Result<PathBuf, String> {
    if !valid_id(id) { return Err("Invalid conversation ID".into()); }
    Ok(dir.join(format!("{id}.json")))
}

fn encoded_size<T: Serialize>(value: &T) -> Result<usize, String> {
    serde_json::to_vec(value).map(|b| b.len()).map_err(|e| format!("Cannot encode conversation: {e}"))
}

fn write_record(dir: &Path, record: &ConversationRecord, replace: bool) -> Result<(), String> {
    let bytes = serde_json::to_vec(record).map_err(|e| format!("Cannot encode conversation: {e}"))?;
    if bytes.len() > RECORD_LIMIT { return Err("Conversation is full (8 MiB); start a new conversation".into()); }
    check_chain(dir)?;
    let target = record_path(dir, &record.id)?;
    let (temp, mut output) = temp_file(dir)?;
    let result = (|| {
        output.write_all(&bytes).and_then(|_| output.sync_all())
            .map_err(|e| format!("Cannot save conversation: {e}"))?;
        drop(output);
        check_chain(&target)?;
        match safe_metadata(&target)? {
            Some(meta) if replace && meta.is_file() && !is_hardlinked(&target, &meta) => replace_file(&temp, &target),
            None if !replace => create_file(&temp, &target),
            Some(_) if !replace => Err("Conversation ID already exists".into()),
            _ => Err("Conversation target is not an independent regular file".into()),
        }
    })();
    let _ = fs::remove_file(&temp);
    result
}

fn request_id_valid(id: &str) -> bool { !id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control) }
fn terminal(state: &str) -> bool { matches!(state, "completed" | "cancelled" | "limited" | "failed" | "interrupted") }

fn read_record(dir: &Path, id: &str) -> Result<ConversationRecord, String> {
    let target = record_path(dir, id)?;
    check_chain(&target)?;
    let meta = safe_metadata(&target)?.ok_or("Conversation not found")?;
    if !meta.is_file() || is_hardlinked(&target, &meta) { return Err("Conversation is not an independent regular file".into()); }
    if meta.len() > RECORD_LIMIT as u64 { return Err("Conversation exceeds 8 MiB".into()); }
    let mut file = File::open(&target).map_err(|e| format!("Cannot open conversation: {e}"))?;
    if !opened_file_within_workspace(&file, dir) { return Err("Conversation file changed unexpectedly".into()); }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    (&mut file).take(RECORD_LIMIT as u64 + 1).read_to_end(&mut bytes)
        .map_err(|e| format!("Cannot read conversation: {e}"))?;
    if bytes.len() > RECORD_LIMIT { return Err("Conversation exceeds 8 MiB".into()); }
    let record: ConversationRecord = serde_json::from_slice(&bytes).map_err(|e| format!("Invalid conversation: {e}"))?;
    if record.schema_version != 1 || record.id != id || !matches!(record.mode.as_str(), "graph" | "workflow")
        || record.title.len() > 512 || record.turns.len() > MAX_TURNS { return Err("Invalid conversation metadata".into()); }
    let mut ids = HashSet::new();
    for turn in &record.turns {
        if !request_id_valid(&turn.id) || !ids.insert(&turn.id) || turn.prompt.len() > MAX_PROMPT
            || !(turn.state == "running" || terminal(&turn.state)) || turn.run_ids.len() > MAX_RUN_IDS
            || turn.run_ids.iter().any(|id| !valid_id(id)) || turn.events.len() > MAX_EVENTS
            || turn.pending_tools.len() > MAX_INTENTS
            || turn.reply.as_ref().is_some_and(|r| r.request_id != turn.id || !terminal(&r.state) || r.state == "interrupted") {
            return Err("Invalid conversation turn metadata".into());
        }
    }
    Ok(record)
}

fn display_record(mut record: ConversationRecord, dir: &Path, active: &HashSet<ActiveKey>) -> ConversationRecord {
    for turn in &mut record.turns {
        if turn.state == "running" && !active.contains(&(dir.to_owned(), record.id.clone(), turn.id.clone())) {
            turn.state = "interrupted".into();
            turn.error = Some("Previous turn was interrupted; pending tools were not replayed".into());
        }
    }
    record
}

fn summary(record: &ConversationRecord) -> ConversationSummary {
    ConversationSummary { id: record.id.clone(), title: record.title.clone(), mode: record.mode.clone(),
        updated_at_ms: record.updated_at_ms, turn_count: record.turns.len(),
        last_state: record.turns.last().map(|t| t.state.clone()).unwrap_or_else(|| "empty".into()) }
}

fn page(mut record: ConversationRecord, before: Option<usize>, limit: Option<usize>) -> Result<ConversationDetail, String> {
    let count = record.turns.len();
    let end = before.unwrap_or(count);
    if end > count { return Err("Invalid conversation page cursor".into()); }
    let limit = limit.unwrap_or(20);
    if !(1..=20).contains(&limit) { return Err("Conversation page size must be 1 to 20".into()); }
    let start = end.saturating_sub(limit);
    let last_state = record.turns.last().map(|t| t.state.clone()).unwrap_or_else(|| "empty".into());
    let turns = record.turns.drain(start..end).collect();
    Ok(ConversationDetail { id: record.id, mode: record.mode, title: record.title,
        created_at_ms: record.created_at_ms, updated_at_ms: record.updated_at_ms,
        turn_count: count, last_state, turns, before: (start > 0).then_some(start), has_more: start > 0 })
}

impl ConversationStore {
    pub(crate) fn create(&self, spaces: &ToolWorkspaces, app_data: &Path, mode: &str, title: Option<&str>) -> Result<ConversationDetail, String> {
        if !matches!(mode, "graph" | "workflow") { return Err("Invalid conversation mode".into()); }
        let title = title.unwrap_or("New conversation");
        if title.trim().is_empty() || title.len() > 512 { return Err("Conversation title must be 1 to 512 bytes".into()); }
        let _active = self.active.lock().map_err(|_| "Conversation store unavailable")?;
        let dir = storage_dir(spaces, app_data, true)?;
        let now = time_ms()?;
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| "System clock is before Unix epoch")?.as_nanos();
        let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
        let id = format!("{:x}", Sha256::digest(format!("{nonce}-{}-{serial}-{}", std::process::id(), dir.display()).as_bytes()));
        let record = ConversationRecord { schema_version: 1, id, mode: mode.into(), title: title.into(),
            created_at_ms: now, updated_at_ms: now, turns: Vec::new() };
        write_record(&dir, &record, false)?;
        page(record, None, None)
    }

    pub(crate) fn list(&self, spaces: &ToolWorkspaces, app_data: &Path) -> Result<ConversationList, String> {
        let active = self.active.lock().map_err(|_| "Conversation store unavailable")?;
        let dir = storage_dir(spaces, app_data, false)?;
        let mut list = ConversationList { records: Vec::new(), warnings: Vec::new(), truncated: false };
        let Some(meta) = safe_metadata(&dir)? else { return Ok(list); };
        if !meta.is_dir() { return Err("Conversation storage is not a directory".into()); }
        let entries = fs::read_dir(&dir).map_err(|e| format!("Cannot list conversations: {e}"))?;
        let mut read_bytes = 0;
        for (index, entry) in entries.enumerate() {
            if index >= LIST_SCAN || list.records.len() >= LIST_RECORDS { list.truncated = true; break; }
            let entry = match entry { Ok(entry) => entry, Err(e) => { list.warnings.push(format!("Cannot inspect conversation: {e}")); continue; } };
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_suffix(".json").filter(|id| valid_id(id)) else { continue; };
            let size = match safe_metadata(&entry.path()) {
                Ok(Some(meta)) => meta.len(),
                _ => { list.warnings.push(format!("Conversation {id} could not be inspected")); continue; }
            };
            if size <= RECORD_LIMIT as u64 {
                if read_bytes + size > LIST_BYTES { list.truncated = true; break; }
                read_bytes += size;
            }
            match read_record(&dir, id) {
                Ok(record) => list.records.push(summary(&display_record(record, &dir, &active))),
                Err(error) => list.warnings.push(format!("Conversation {id}: {error}")),
            }
        }
        list.records.sort_by(|a, b| b.updated_at_ms.cmp(&a.updated_at_ms).then_with(|| a.id.cmp(&b.id)));
        Ok(list)
    }

    pub(crate) fn load(&self, spaces: &ToolWorkspaces, app_data: &Path, id: &str, before: Option<usize>, limit: Option<usize>) -> Result<ConversationDetail, String> {
        let active = self.active.lock().map_err(|_| "Conversation store unavailable")?;
        let dir = storage_dir(spaces, app_data, false)?;
        page(display_record(read_record(&dir, id)?, &dir, &active), before, limit)
    }

    pub(crate) fn begin_turn(self: &Arc<Self>, spaces: &ToolWorkspaces, app_data: &Path, id: &str,
        mode: &str, request_id: &str, prompt: &str, context_graph: Option<Value>) -> Result<ConversationTurnGuard, String> {
        if !request_id_valid(request_id) { return Err("Invalid conversation request ID".into()); }
        if prompt.trim().is_empty() || prompt.len() > MAX_PROMPT { return Err("Prompt must be 1 to 16384 bytes".into()); }
        if context_graph.as_ref().is_some_and(|v| encoded_size(v).unwrap_or(usize::MAX) > MAX_CONTEXT) {
            return Err("Saved graph context exceeds 128 KiB".into());
        }
        let mut active = self.active.lock().map_err(|_| "Conversation store unavailable")?;
        let dir = storage_dir(spaces, app_data, false)?;
        let mut record = read_record(&dir, id)?;
        if record.mode != mode { return Err("Conversation mode cannot be changed".into()); }
        if record.turns.iter().any(|t| t.id == request_id) { return Err("This request ID has already been recorded; start a new turn".into()); }
        if active.iter().any(|(d, conversation, _)| d == &dir && conversation == id) { return Err("Conversation already has an active turn".into()); }
        if record.turns.len() >= MAX_TURNS || encoded_size(&record)? + TURN_RESERVE > RECORD_LIMIT {
            return Err("Conversation is full; start a new conversation".into());
        }
        let displayed = display_record(record.clone(), &dir, &active);
        let prior_turns = displayed.turns.into_iter().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect();
        // Persist previous interruption only as part of this explicit write.
        for turn in &mut record.turns {
            if turn.state == "running" { turn.state = "interrupted".into();
                turn.error = Some("Previous turn was interrupted; pending tools were not replayed".into()); }
        }
        let now = time_ms()?;
        if record.turns.is_empty() && record.title == "New conversation" { record.title = prompt.chars().take(80).collect(); }
        let mut graph = context_graph;
        if let Some(graph) = &mut graph { remove_private_fields(graph); }
        record.turns.push(ConversationTurn { id: request_id.into(), prompt: prompt.into(), context_graph: graph,
            started_at_ms: now, finished_at_ms: None, state: "running".into(), reply: None,
            error: None, events: Vec::new(), run_ids: Vec::new(), pending_tools: Vec::new() });
        record.updated_at_ms = now;
        write_record(&dir, &record, true)?;
        active.insert((dir.clone(), id.into(), request_id.into()));
        Ok(ConversationTurnGuard { store: self.clone(), dir, conversation_id: id.into(),
            request_id: request_id.into(), finished: false, prior_turns })
    }
}

fn remove_private_fields(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|key, _| !matches!(key.to_ascii_lowercase().as_str(),
                "reasoning_content" | "reasoning" | "api_key" | "apikey" | "authorization" | "proxy-authorization" | "authentication"));
            for value in map.values_mut() { remove_private_fields(value); }
        }
        Value::Array(values) => for value in values { remove_private_fields(value); },
        _ => {}
    }
}

fn collect_run_ids(value: &Value, run_ids: &mut Vec<String>) {
    match value {
        Value::Object(map) => for (key, value) in map {
            if key == "run_id" { if let Some(id) = value.as_str().filter(|id| valid_id(id)) {
                if !run_ids.iter().any(|old| old == id) { run_ids.push(id.into()); }
            } }
            else if key == "run_ids" { if let Some(ids) = value.as_array() { for id in ids {
                if let Some(id) = id.as_str().filter(|id| valid_id(id)) {
                    if !run_ids.iter().any(|old| old == id) { run_ids.push(id.into()); }
                }
            } } }
            collect_run_ids(value, run_ids);
        },
        Value::Array(values) => for value in values { collect_run_ids(value, run_ids); },
        _ => {}
    }
}

fn bounded_text(text: &str, limit: usize) -> (String, Option<usize>) {
    if text.len() <= limit { return (text.into(), None); }
    let mut end = limit;
    while !text.is_char_boundary(end) { end -= 1; }
    (text[..end].into(), Some(text.len() - end))
}

fn omitted_value(bytes: usize) -> Value { json!({"omitted":true,"original_bytes":bytes,"reason":"conversation_storage_limit"}) }

fn visible_events(value: Value, run_ids: &mut Vec<String>) -> Result<Vec<StoredEvent>, String> {
    let values = value.as_array().ok_or("Conversation events must be an array")?;
    if values.len() > MAX_EVENTS { return Err("Too many conversation events".into()); }
    let mut events = Vec::with_capacity(values.len());
    let mut remaining = EVENTS_BYTES;
    for value in values {
        collect_run_ids(value, run_ids);
        let kind = value.get("kind").and_then(Value::as_str).ok_or("Conversation event kind is missing")?;
        if !matches!(kind, "input" | "assistant" | "tool" | "status") { return Err("Invalid conversation event kind".into()); }
        let mut event = StoredEvent { kind: kind.into(), text: value.get("text").and_then(Value::as_str).map(str::to_owned),
            tool: value.get("tool").and_then(Value::as_str).map(str::to_owned), arguments: value.get("arguments").cloned(),
            result: value.get("result").cloned(), success: value.get("success").and_then(Value::as_bool), omitted_bytes: None };
        if event.tool.as_ref().is_some_and(|name| name.len() > 128 || name.chars().any(char::is_control)) {
            return Err("Invalid stored tool name".into());
        }
        if let Some(v) = &mut event.arguments { remove_private_fields(v); }
        if let Some(v) = &mut event.result { remove_private_fields(v); }
        let original = encoded_size(&event)?;
        let limit = EVENT_BYTES.min(remaining);
        if original > limit {
            event.omitted_bytes = Some(original);
            event.text = event.text.as_deref().map(|text| bounded_text(text, 64).0);
            event.arguments = event.arguments.as_ref().map(|v| omitted_value(encoded_size(v).unwrap_or(usize::MAX)));
            event.result = event.result.as_ref().map(|v| omitted_value(encoded_size(v).unwrap_or(usize::MAX)));
        }
        remaining = remaining.saturating_sub(encoded_size(&event)?);
        events.push(event);
    }
    Ok(events)
}

fn merge_runs(existing: &mut Vec<String>, added: Vec<String>) -> Result<(), String> {
    for id in added {
        if !valid_id(&id) { return Err("Invalid conversation run ID".into()); }
        if !existing.contains(&id) { existing.push(id); }
    }
    if existing.len() > MAX_RUN_IDS { return Err("Too many conversation run IDs".into()); }
    Ok(())
}

impl ConversationTurnGuard {
    fn key(&self) -> ActiveKey { (self.dir.clone(), self.conversation_id.clone(), self.request_id.clone()) }

    pub fn checkpoint(&mut self, events: Value, run_ids: Vec<String>, mut pending_tools: Vec<ToolIntent>) -> Result<(), String> {
        if self.finished { return Err("Conversation turn is already finished".into()); }
        if pending_tools.len() > MAX_INTENTS { return Err("Too many pending conversation tools".into()); }
        let active = self.store.active.lock().map_err(|_| "Conversation store unavailable")?;
        if !active.contains(&self.key()) { return Err("Conversation turn is not active".into()); }
        let mut record = read_record(&self.dir, &self.conversation_id)?;
        let turn = record.turns.last_mut().filter(|t| t.id == self.request_id && t.state == "running")
            .ok_or("Conversation turn is not current")?;
        let mut ids = turn.run_ids.clone();
        merge_runs(&mut ids, run_ids)?;
        let events = visible_events(events, &mut ids)?;
        let mut tool_ids = HashSet::new();
        for intent in &mut pending_tools {
            if intent.id.is_empty() || intent.id.len() > 256 || intent.name.is_empty() || intent.name.len() > 128
                || intent.id.chars().any(char::is_control) || intent.name.chars().any(char::is_control)
                || !tool_ids.insert(intent.id.clone()) { return Err("Invalid pending conversation tool".into()); }
            if let Some(arguments) = &mut intent.arguments {
                collect_run_ids(arguments, &mut ids);
                remove_private_fields(arguments);
                let size = encoded_size(arguments)?;
                if size > INTENT_BYTES { *arguments = omitted_value(size); intent.omitted_bytes = Some(size); }
            }
        }
        if ids.len() > MAX_RUN_IDS { return Err("Too many conversation run IDs".into()); }
        turn.events = events;
        turn.pending_tools = pending_tools;
        turn.run_ids = ids;
        record.updated_at_ms = time_ms()?;
        write_record(&self.dir, &record, true)
    }

    pub fn finish(&mut self, value: Value, run_ids: Vec<String>) -> Result<(), String> {
        if self.finished { return Err("Conversation turn is already finished".into()); }
        let mut active = self.store.active.lock().map_err(|_| "Conversation store unavailable")?;
        if !active.contains(&self.key()) { return Err("Conversation turn is not active".into()); }
        let mut record = read_record(&self.dir, &self.conversation_id)?;
        let turn = record.turns.last_mut().filter(|t| t.id == self.request_id && t.state == "running")
            .ok_or("Conversation turn is not current")?;
        let id = value.get("request_id").and_then(Value::as_str).ok_or("Saved reply request ID is missing")?;
        let state = value.get("state").and_then(Value::as_str).ok_or("Saved reply state is missing")?;
        if id != self.request_id || !terminal(state) || state == "interrupted" { return Err("Invalid saved conversation reply".into()); }
        if state == "completed" && !turn.pending_tools.is_empty() {
            return Err("A completed conversation turn cannot have pending tools".into());
        }
        let text = value.get("text").and_then(Value::as_str).ok_or("Saved reply text is missing")?;
        let (text, omitted_text_bytes) = bounded_text(text, MAX_TEXT);
        let mut ids = turn.run_ids.clone();
        merge_runs(&mut ids, run_ids)?;
        let events = visible_events(value.get("events").cloned().unwrap_or_else(|| json!([])), &mut ids)?;
        if ids.len() > MAX_RUN_IDS { return Err("Too many conversation run IDs".into()); }
        let reply = StoredReply { request_id: id.into(), state: state.into(), text, events,
            model_calls: value.get("model_calls").and_then(Value::as_u64).unwrap_or(0).min(16) as usize,
            tool_calls: value.get("tool_calls").and_then(Value::as_u64).unwrap_or(0).min(40) as usize,
            omitted_text_bytes };
        let now = time_ms()?;
        turn.state = state.into();
        turn.error = (state == "failed").then(|| reply.text.clone());
        // Final events are stored once; the reply owns the complete visible transcript.
        turn.events.clear();
        turn.reply = Some(reply);
        turn.run_ids = ids;
        turn.finished_at_ms = Some(now);
        record.updated_at_ms = now;
        write_record(&self.dir, &record, true)?;
        active.remove(&self.key());
        self.finished = true;
        Ok(())
    }
}

impl Drop for ConversationTurnGuard {
    fn drop(&mut self) {
        // No I/O from Drop: panic/cancellation releases the lease, and reads display
        // the durable unfinished intent as interrupted without ever executing it.
        let mut active = self.store.active.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        active.remove(&self.key());
    }
}

#[cfg(test)]
#[path = "conversation_store_tests.rs"]
mod tests;
