//! Read-only, bounded views of the current project's persisted run records.
use crate::run_records::{RunRecord, RunStore, RunSummary};
use crate::tool_workspaces::ToolWorkspaces;
use serde_json::{json, Map, Value};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub const NAMES: &[&str] = &["runs_list", "runs_read", "runs_check_files"];
const INLINE_BYTES: usize = 12 * 1024;
// Reserve space for the existing outer {ok,data} tool-result envelope.
const REPLY_BYTES: usize = 24 * 1024 - 256;
const PAGE_BYTES: usize = 18 * 1024;

pub fn definitions() -> Vec<Value> {
    vec![
        definition("runs_list", "List current-project run summaries, newest first. Read-only historical evidence, not proof of current files. Source scan limits can make history_incomplete true even on an empty final page. Use the returned keyset cursor for the next page.", json!({
            "kind":{"type":"string","enum":["graph","workflow"]},
            "state":{"type":"string","enum":["running","succeeded","failed","cancelled","interrupted","limited","unknown"]},
            "parent_id":{"type":["string","null"],"description":"Exact parent run ID; null selects root runs."},
            "limit":{"type":"integer","minimum":1,"maximum":20,"default":10},
            "cursor":{"type":"object","additionalProperties":false,"properties":{"started_at_ms":{"type":"integer","minimum":0},"id":{"type":"string"}},"required":["started_at_ms","id"]}
        }), &[]),
        definition("runs_read", "Read one stored run section. Small selections return complete inline data. Large objects/arrays return a directory of JSON Pointers, not a complete configuration or Graph. Follow a pointer relative to the section to inspect a child. Large strings return explicitly incomplete character slices. offset/limit page directory entries or Unicode characters. Never interpret historical results as a fresh execution.", json!({
            "id":{"type":"string"},
            "section":{"type":"string","enum":["summary","configuration","result","files","error"],"default":"summary"},
            "pointer":{"type":"string","description":"RFC 6901 JSON Pointer relative to configuration/result/files only; empty selects the section root."},
            "offset":{"type":"integer","minimum":0,"default":0},
            "limit":{"type":"integer","minimum":1,"maximum":2048,"description":"Directory entries: default20/max40; text characters: default2048/max2048."}
        }), &["id"]),
        definition("runs_check_files", "Check current referenced files against recorded verified hashes using bounded existing safety and hash budgets. available requires verified hashes. changed/missing/unverified are distinct. Checks describe current files at checked_at_ms and may change; run records are not backups. Paginate the file-status reply.", json!({
            "id":{"type":"string"},"offset":{"type":"integer","minimum":0,"default":0},
            "limit":{"type":"integer","minimum":1,"maximum":40,"default":20}
        }), &["id"]),
    ]
}

fn definition(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({"type":"function","function":{"name":name,"description":description,"parameters":{
        "type":"object","properties":properties,"required":required,"additionalProperties":false
    }}})
}

fn args<'a>(value: &'a Value, allowed: &[&str]) -> Result<&'a Map<String, Value>, String> {
    let object = value.as_object().ok_or("Run history arguments must be an object")?;
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err("Unknown run history argument; only the current project is accessible".into());
    }
    Ok(object)
}

fn optional_string<'a>(args: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>, String> {
    args.get(key).map(|v| v.as_str().ok_or_else(|| format!("Invalid {key}"))).transpose()
}

fn id_valid(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn id_arg(args: &Map<String, Value>) -> Result<&str, String> {
    let id = optional_string(args, "id")?.ok_or("Missing run ID")?;
    if !id_valid(id) { return Err("Invalid run ID".into()); }
    Ok(id)
}

fn number(args: &Map<String, Value>, key: &str, default: usize, maximum: usize) -> Result<usize, String> {
    let Some(value) = args.get(key) else { return Ok(default); };
    let value = value.as_u64().and_then(|v| usize::try_from(v).ok()).ok_or_else(|| format!("Invalid {key}"))?;
    if value > maximum || (key == "limit" && value == 0) { return Err(format!("{key} exceeds supported range")); }
    Ok(value)
}

fn bytes(value: &Value) -> usize { serde_json::to_vec(value).map_or(usize::MAX, |v| v.len()) }

fn preview(text: &str, limit: usize) -> (String, bool) {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) { end -= 1; }
    (text[..end].to_owned(), end < text.len())
}

fn reply(tool: &str) -> Value {
    json!({"schema_version":1,"scope":"current_project","tool":tool,"complete":true,"warnings":[]})
}

fn summary(value: &RunSummary) -> Value {
    let (name, cut) = preview(&value.name, 512);
    let parent_valid = value.parent_id.as_deref().is_none_or(id_valid);
    json!({"id":value.id,"kind":value.kind,"origin":value.origin,
        "parent_id":if parent_valid { value.parent_id.clone() } else { None },
        "name":name,"started_at_ms":value.started_at_ms,"finished_at_ms":value.finished_at_ms,
        "duration_ms":value.duration_ms,"state":value.state,
        "omitted_fields":if !parent_valid { vec!["parent_id"] } else { vec![] },"name_truncated":cut})
}

fn record_summary(record: &RunRecord) -> Value {
    let mut value = summary(&RunSummary { id:record.id.clone(), kind:record.kind.clone(),
        origin:record.origin.clone(), parent_id:record.parent_id.clone(), name:record.name.clone(),
        started_at_ms:record.started_at_ms, finished_at_ms:record.finished_at_ms,
        duration_ms:record.duration_ms, state:record.state.clone() });
    let (warning, cut) = record.recording_warning.as_deref().map(|w| preview(w, 512)).unwrap_or_default();
    value["recording_warning"] = if record.recording_warning.is_some() { json!(warning) } else { Value::Null };
    value["recording_warning_truncated"] = json!(cut);
    let (error, error_cut) = record.error.as_deref().map(|e| preview(e, 512)).unwrap_or_default();
    value["error"] = if record.error.is_some() { json!(error) } else { Value::Null };
    value["error_truncated"] = json!(error_cut);
    value
}

fn list(store: &RunStore, spaces: &ToolWorkspaces, app_data: &Path, input: &Value) -> Result<Value, String> {
    let input = args(input, &["kind","state","parent_id","limit","cursor"])?;
    let kind = optional_string(input, "kind")?;
    if kind.is_some_and(|v| !matches!(v, "graph" | "workflow")) { return Err("Invalid run kind".into()); }
    let state = optional_string(input, "state")?;
    if state.is_some_and(|v| !matches!(v, "running" | "succeeded" | "failed" | "cancelled" | "interrupted" | "limited" | "unknown")) {
        return Err("Invalid run state".into());
    }
    let parent = input.get("parent_id");
    if parent.is_some_and(|v| !v.is_null() && !v.as_str().is_some_and(id_valid)) { return Err("Invalid parent run ID".into()); }
    let limit = number(input, "limit", 10, 20)?;
    let cursor = if let Some(cursor) = input.get("cursor") {
        let cursor = args(cursor, &["started_at_ms","id"])?;
        let time = cursor.get("started_at_ms").and_then(Value::as_u64).ok_or("Invalid history cursor")?;
        Some((time, id_arg(cursor)?))
    } else { None };
    let source = store.list(spaces, app_data).map_err(|_| "Cannot read current-project run history")?;
    // RunStore sorts both fields descending; a cursor is independent of insertions before it.
    let matches: Vec<_> = source.records.iter().filter(|r| {
        kind.is_none_or(|v| r.kind == v) && state.is_none_or(|v| r.state == v)
            && parent.is_none_or(|v| if v.is_null() { r.parent_id.is_none() } else { r.parent_id.as_deref() == v.as_str() })
            && cursor.is_none_or(|(time, id)| r.started_at_ms < time || (r.started_at_ms == time && r.id.as_str() < id))
    }).collect();
    let mut records = Vec::new();
    let mut size = 0;
    let mut field_omissions = false;
    for record in matches.iter().take(limit) {
        let item = summary(record);
        let item_bytes = bytes(&item);
        if size + item_bytes > PAGE_BYTES { break; }
        field_omissions |= item["name_truncated"] == true || !item["omitted_fields"].as_array().unwrap().is_empty();
        size += item_bytes;
        records.push(item);
    }
    let has_more = records.len() < matches.len();
    let next = if has_more { records.last().map(|r| json!({"started_at_ms":r["started_at_ms"],"id":r["id"]})) } else { None };
    let incomplete = source.truncated || !source.warnings.is_empty();
    let warnings: Vec<_> = source.warnings.iter().take(4).map(|w| preview(w, 512).0).collect();
    let mut output = reply("runs_list");
    output["complete"] = json!(!incomplete && !has_more && !field_omissions);
    output["records"] = json!(records);
    output["next_cursor"] = json!(next);
    output["history_incomplete"] = json!(incomplete);
    output["warnings"] = json!(warnings);
    output["warnings_omitted"] = json!(source.warnings.len().saturating_sub(4));
    output["source_records"] = json!(source.records.len());
    if incomplete {
        output["warnings"].as_array_mut().unwrap().push(json!("History source is partial or has unreadable entries; an empty page does not establish no matching runs."));
    }
    Ok(output)
}

fn validate_pointer(pointer: &str) -> Result<(), String> {
    if pointer.len() > 4096 || (!pointer.is_empty() && !pointer.starts_with('/')) { return Err("Invalid JSON Pointer".into()); }
    let mut chars = pointer.chars();
    while let Some(ch) = chars.next() {
        if ch == '~' && !matches!(chars.next(), Some('0' | '1')) { return Err("Invalid JSON Pointer escape".into()); }
    }
    Ok(())
}

fn value_type(value: &Value) -> &'static str {
    match value { Value::Null => "null", Value::Bool(_) => "boolean", Value::Number(_) => "number",
        Value::String(_) => "string", Value::Array(_) => "array", Value::Object(_) => "object" }
}

fn directory_entry(pointer: &str, token: &str, value: &Value) -> Value {
    let child = format!("{pointer}/{}", token.replace('~', "~0").replace('/', "~1"));
    let accessible = child.len() <= 4096 && bytes(&json!(child)) <= 4096;
    let mut item = json!({"pointer":if accessible { Some(child) } else { None },
        "type":value_type(value),"size_bytes":bytes(value),"read_available":accessible});
    if !accessible {
        item["key_preview"] = json!(preview(token, 128).0);
        item["pointer_omitted"] = json!(true);
    }
    match value {
        Value::Array(a) => item["child_count"] = json!(a.len()),
        Value::Object(o) => item["child_count"] = json!(o.len()),
        Value::String(s) => item["chars"] = json!(s.chars().count()),
        _ => {},
    }
    item
}

fn read(store: &RunStore, spaces: &ToolWorkspaces, app_data: &Path, input: &Value) -> Result<Value, String> {
    let input = args(input, &["id","section","pointer","offset","limit"])?;
    let id = id_arg(input)?;
    let section = optional_string(input, "section")?.unwrap_or("summary");
    if !matches!(section, "summary" | "configuration" | "result" | "files" | "error") { return Err("Invalid run section".into()); }
    let pointer = optional_string(input, "pointer")?.unwrap_or("");
    if input.contains_key("pointer") && !matches!(section, "configuration" | "result" | "files") {
        return Err("JSON Pointer is only supported for configuration, result and files".into());
    }
    validate_pointer(pointer)?;
    let offset = number(input, "offset", 0, usize::MAX)?;
    let record = store.load(spaces, app_data, id).map_err(|_| "Run record unavailable in current project")?;
    let recording_warning = record.recording_warning.as_deref().map(|w| preview(w, 512));
    let selected = match section {
        "summary" => record_summary(&record), "configuration" => record.configuration,
        "result" => record.result.unwrap_or(Value::Null), "error" => json!(record.error),
        "files" => serde_json::to_value(record.files).map_err(|_| "Cannot encode file references")?,
        _ => unreachable!(),
    };
    let value = selected.pointer(pointer).ok_or("JSON Pointer does not exist in selected section")?;
    let mut output = reply("runs_read");
    output["id"] = json!(id); output["section"] = json!(section); output["pointer"] = json!(pointer);
    output["size_bytes"] = json!(bytes(value));
    if let Some((warning, cut)) = recording_warning {
        output["recording_warning"] = json!(warning);
        output["recording_warning_truncated"] = json!(cut);
    }
    let paging = input.contains_key("offset") || input.contains_key("limit");
    if section == "summary" || (bytes(value) <= INLINE_BYTES && !paging) {
        if section == "summary" {
            number(input, "limit", 20, 2048)?;
            if offset != 0 { return Err("Summary does not support an offset".into()); }
        }
        output["view"] = json!("inline"); output["data"] = value.clone();
        if section == "summary" && (value["name_truncated"] == true || value["recording_warning_truncated"] == true || value["error_truncated"] == true
            || !value["omitted_fields"].as_array().unwrap().is_empty()) {
            output["complete"] = json!(false);
            output["warnings"] = json!(["Summary fields exceeded limits and were explicitly shortened or omitted."]);
        }
    } else if let Some(text) = value.as_str() {
        let limit = number(input, "limit", 2048, 2048)?;
        let total = text.chars().count();
        if offset > total { return Err("Text offset exceeds selected value".into()); }
        let data: String = text.chars().skip(offset).take(limit).collect();
        let end = offset + data.chars().count();
        output["view"] = json!("text"); output["data"] = json!(data);
        output["offset"] = json!(offset); output["total_chars"] = json!(total);
        output["next_offset"] = if end < total { json!(end) } else { Value::Null };
        output["complete"] = json!(offset == 0 && end == total);
        output["warnings"] = if offset == 0 && end == total { json!([]) } else { json!(["This is a character slice, not the complete selected value."]) };
    } else if value.is_object() || value.is_array() {
        let limit = number(input, "limit", 20, 40)?;
        let total = value.as_object().map(Map::len).unwrap_or_else(|| value.as_array().unwrap().len());
        if offset > total { return Err("Directory offset exceeds selected value".into()); }
        let mut entries = Vec::new();
        let mut size = 0;
        let children: Box<dyn Iterator<Item = (String, &Value)> + '_> = match value {
            Value::Object(o) => Box::new(o.iter().map(|(k,v)| (k.clone(),v))),
            Value::Array(a) => Box::new(a.iter().enumerate().map(|(i,v)| (i.to_string(),v))),
            _ => unreachable!(),
        };
        for (token, child) in children.skip(offset).take(limit) {
            let entry = directory_entry(pointer, &token, child);
            if size + bytes(&entry) > PAGE_BYTES { break; }
            size += bytes(&entry); entries.push(entry);
        }
        let end = offset + entries.len();
        output["view"] = json!("directory"); output["entries"] = json!(entries);
        output["offset"] = json!(offset); output["total_items"] = json!(total);
        output["next_offset"] = if end < total { json!(end) } else { Value::Null };
        output["complete"] = json!(false);
        output["warnings"] = json!(["This is a directory of child metadata, not the complete selected JSON value or configuration. Follow a child pointer to read its value."]);
    } else {
        number(input, "limit", 1, 2048)?;
        if offset != 0 { return Err("Scalar values do not support an offset".into()); }
        output["view"] = json!("inline"); output["data"] = value.clone();
    }
    if output["recording_warning_truncated"] == true { output["complete"] = json!(false); }
    Ok(output)
}

fn check_files(store: &RunStore, spaces: &ToolWorkspaces, app_data: &Path, input: &Value) -> Result<Value, String> {
    let input = args(input, &["id","offset","limit"])?;
    let id = id_arg(input)?;
    let offset = number(input, "offset", 0, usize::MAX)?;
    let limit = number(input, "limit", 20, 40)?;
    let checks = store.check_files(spaces, app_data, id).map_err(|_| "Run file references unavailable in current project")?;
    if offset > checks.len() { return Err("File offset exceeds reference count".into()); }
    let checked_at_ms = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| "Invalid system clock")?.as_millis() as u64;
    let mut files = Vec::new();
    let mut size = 0;
    let mut omissions = false;
    for file in checks.iter().skip(offset).take(limit) {
        let (path, cut) = preview(&file.path, 1024);
        let (message, message_cut) = file.message.as_deref().map(|v| preview(v, 512)).unwrap_or_default();
        let item = json!({"space":preview(&file.space,32).0,"path":path,"path_truncated":cut,
            "role":preview(&file.role,32).0,"status":file.status,
            "message":if file.message.is_some() { Some(message) } else { None },"message_truncated":message_cut});
        if size + bytes(&item) > PAGE_BYTES { break; }
        omissions |= cut || message_cut;
        size += bytes(&item); files.push(item);
    }
    let end = offset + files.len();
    let mut output = reply("runs_check_files");
    output["id"] = json!(id); output["files"] = json!(files); output["offset"] = json!(offset);
    output["total_items"] = json!(checks.len()); output["checked_at_ms"] = json!(checked_at_ms);
    output["not_backup"] = json!(true);
    output["complete"] = json!(offset == 0 && end == checks.len() && !omissions);
    output["next_offset"] = if end < checks.len() { json!(end) } else { Value::Null };
    output["warnings"] = json!(["Current file checks are time-dependent. Recorded paths and hashes are references, not backups; available requires verified hashes."]);
    Ok(output)
}

pub fn dispatch(store: &RunStore, spaces: &ToolWorkspaces, app_data: &Path, name: &str, args: &Value) -> Result<Value, String> {
    let output = match name {
        "runs_list" => list(store, spaces, app_data, args),
        "runs_read" => read(store, spaces, app_data, args),
        "runs_check_files" => check_files(store, spaces, app_data, args),
        _ => Err("Unknown run history tool".into()),
    }?;
    if bytes(&output) > REPLY_BYTES {
        return Err("Run history reply exceeds 24 KiB; narrow the selected pointer or page size. No oversized value was returned.".into());
    }
    Ok(output)
}

#[cfg(test)]
#[path = "run_history_tools_tests.rs"]
mod tests;
