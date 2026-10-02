use crate::backend::BackendManager;
use crate::run_records::{RunDraft, RunFileDraft, RunStore};
use crate::run_history_tools;
use crate::tool_workspaces::ToolWorkspaces;
use crate::workflow::{self, WorkflowHost};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::path::PathBuf;
use std::path::Path;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::time::{Duration, Instant};

const GRAPH_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) fn valid_mode(mode: &str) -> bool { matches!(mode, "graph" | "workflow") }

pub(crate) fn names(mode: &str) -> Vec<String> {
    if !valid_mode(mode) { return Vec::new(); }
    let mut names = vec!["workspace_list", "file_read_text", "file_write_text", "file_delete",
        "file_copy_to_ai", "file_export", "directory_create", "audio_inspect", "nodes_list", "graph_validate"];
    if mode == "workflow" { names.extend(["graph_run", "workflow_validate", "workflow_run"]); }
    names.extend(run_history_tools::NAMES.iter().copied());
    names.into_iter().map(str::to_owned).collect()
}

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}

pub(crate) fn definitions(mode: &str) -> Vec<Value> {
    if !valid_mode(mode) { return Vec::new(); }
    let space = json!({"type":"string","enum":["user","ai"]});
    let path = json!({"type":"string","description":"Relative path in the named workspace. No absolute paths or parent traversal."});
    let graph = json!({"type":"object"});
    let execution = schema(json!({"graph":graph,"mode":{"type":"string","enum":["offline","streaming"]},"options":{"type":"object"}}), &["graph","mode"]);
    let validation = schema(json!({"space":space,"graph":{"type":"object"},"mode":{"type":"string","enum":["offline","streaming"]},"options":{"type":"object"}}), &["space","graph","mode"]);
    let rows: Vec<(&str,&str,Value)> = vec![
        ("workspace_list","List one workspace directory in stable name order. offset defaults to 0 and limit to 100 (maximum 200). Entries also have a 16 KiB page byte budget, so a page may be shorter than limit with page_byte_limited=true. Follow next_offset, which advances by actual returned entries, for later pages; truncated means more entries may remain after this page. A scan is bounded to 10000 entries or 2 seconds: partial=true makes total unknown and pages cover only the scanned subset.",schema(json!({"space":space,"path":path,
            "offset":{"type":"integer","minimum":0,"maximum":10000},
            "limit":{"type":"integer","minimum":1,"maximum":200}}), &["space"])),
        ("file_read_text","Read up to 64 KiB of UTF-8 text.",schema(json!({"space":space,"path":path}), &["space","path"])),
        ("file_write_text","Write UTF-8 text in the AI workspace only.",schema(json!({"path":path,"content":{"type":"string"}}), &["path","content"])),
        ("file_delete","Delete a file in the AI workspace only.",schema(json!({"path":path}), &["path"])),
        ("file_copy_to_ai","Copy a user or AI file into the AI workspace.",schema(json!({"source_space":space,"source_path":path,"path":path}), &["source_space","source_path","path"])),
        ("file_export","Copy one AI file to a NEW user file, never overwrite. Prefer one fresh output folder per batch and reuse it for that batch's files. user_path='batch-name/output.wav' safely creates the single top-level folder if missing; deeper missing directories are not created. This is not recursive directory export.",schema(json!({"path":path,"user_path":path}), &["path","user_path"])),
        ("directory_create","Create a directory in the AI workspace ONLY, including missing parents, at most 3 levels below its root (a/b/c). Existing normal directories succeed with created=false. No user-workspace directory access. The same depth limit applies to file_write_text and file_copy_to_ai parent creation.",schema(json!({"path":path}), &["path"])),
        ("audio_inspect","Return PCM16 WAV metadata without audio samples.",schema(json!({"space":space,"path":path}), &["space","path"])),
        ("nodes_list","List available offline and streaming audio nodes.",schema(json!({}), &[])),
        ("graph_validate","Validate a graph whose file paths are relative to the selected workspace.",validation),
        ("graph_run","Run a graph in the AI workspace; outputs remain there.",execution),
        ("workflow_validate","Read a workflow JSON file (64 KiB maximum) and validate its structure without running steps. Workflow v1 uses schema_version:1, inputs:{}, steps:[{id,type}], outputs:{}; steps are set, call, for_each, or if. References use {$ref:\"/inputs/name\"} or earlier /steps/id paths.",schema(json!({"space":space,"path":path}), &["space","path"])),
        ("workflow_run","Run a validated workflow JSON file. inputs overrides only declared input keys. Calls use the existing basic tools; a failed step returns a partial report.",schema(json!({"space":space,"path":path,"inputs":{"type":"object"}}), &["space","path"])),
    ];
    let mut definitions: Vec<Value> = rows.into_iter().filter(|(name,_,_)| mode == "workflow" || !matches!(*name, "graph_run" | "workflow_validate" | "workflow_run"))
        .map(|(name,description,parameters)| json!({"type":"function","function":{"name":name,"description":description,"parameters":parameters}})).collect();
    definitions.extend(run_history_tools::definitions());
    definitions
}

fn arg<'a>(args: &'a Value, key: &str) -> Result<&'a str,String> {
    args.get(key).and_then(Value::as_str).ok_or_else(|| format!("Missing string argument: {key}"))
}

fn fields(args: &Value, required: &[&str], optional: &[&str]) -> Result<(),String> {
    let object = args.as_object().ok_or("Tool arguments must be a JSON object")?;
    for key in object.keys() {
        if !required.contains(&key.as_str()) && !optional.contains(&key.as_str()) {
            return Err(format!("Unknown tool argument: {key}"));
        }
    }
    for key in required { if !object.contains_key(*key) { return Err(format!("Missing tool argument: {key}")); } }
    Ok(())
}

fn ensure_success(reply: Value) -> Result<Value,String> {
    if reply.get("success") == Some(&Value::Bool(true)) {
        reply.get("data").cloned().ok_or("Backend response lacks data".into())
    } else {
        Err(format!("Backend rejected operation: {}", reply.get("errors").cloned().unwrap_or(Value::Null)))
    }
}

// Record only the origins actually used by this graph, not private plugin paths
// or a caller-supplied claim. Catalog metadata comes from the connected engine.
pub(crate) fn graph_plugin_refs(graph: &Value, catalog: &Value) -> Value {
    let definitions = catalog.get("nodes").or_else(|| catalog.pointer("/data/nodes"))
        .and_then(Value::as_array);
    let mut refs = Vec::new();
    if let (Some(nodes),Some(definitions)) = (graph.get("nodes").and_then(Value::as_array),definitions) {
        for node in nodes {
            if let Some(origin) = definitions.iter().find(|d| d["typeId"] == node["type"])
                .and_then(|d| d.get("plugin")).filter(|p| p.is_object()) {
                let mut provenance = serde_json::Map::new();
                for field in ["id","implementation_version","package_sha256","abi","capabilities"] {
                    if let Some(value) = origin.get(field) { provenance.insert(field.into(),value.clone()); }
                }
                refs.push(json!({"node_id":node["id"],"type_id":node["type"],"plugin":provenance}));
            }
        }
    }
    json!(refs)
}

pub(crate) fn graph_file_refs(graph: &Value, catalog: &Value, space: &str) -> Vec<RunFileDraft> {
    let definitions = catalog.get("nodes").and_then(Value::as_array).or_else(|| catalog.get("data").and_then(|v| v.get("nodes")).and_then(Value::as_array));
    let Some(definitions) = definitions else { return Vec::new(); };
    let mut files = Vec::new();
    for node in graph.get("nodes").and_then(Value::as_array).into_iter().flatten() {
        let Some(kind) = node.get("type").and_then(Value::as_str) else { continue; };
        let Some(definition) = definitions.iter().find(|item| item.get("typeId").and_then(Value::as_str) == Some(kind)) else { continue; };
        let Some(parameters) = definition.get("parameters").and_then(Value::as_array) else { continue; };
        for descriptor in parameters {
            if descriptor.get("type").and_then(Value::as_str) != Some("file_path") { continue; }
            let Some(key) = descriptor.get("id").and_then(Value::as_str) else { continue; };
            let Some(path) = node.get("parameters").and_then(|v| v.get(key)).and_then(Value::as_str) else { continue; };
            if path.is_empty() { continue; }
            let role = match kind { "wav_input" | "wav_stream_input" => "input",
                "wav_output" | "wav_stream_output" | "text_output" => "output", _ => "related" };
            files.push(RunFileDraft { space:space.into(), path:path.into(), role:role.into() });
        }
    }
    files
}

pub(crate) fn typed_result_files(result: &Value, space: &str) -> Vec<RunFileDraft> {
    fn visit(value: &Value, space: &str, files: &mut Vec<RunFileDraft>) {
        if value.get("type").and_then(Value::as_str) == Some("FilePath") {
            if let Some(path) = value.get("value").and_then(Value::as_str) {
                if !path.is_empty() { files.push(RunFileDraft { space:space.into(),path:path.into(),role:"output".into() }); }
            }
            return;
        }
        match value {
            Value::Array(items) => for item in items { visit(item,space,files); },
            Value::Object(items) => for item in items.values() { visit(item,space,files); },
            _ => {}
        }
    }
    let mut files = Vec::new();
    visit(result,space,&mut files);
    files
}

pub(crate) fn normalize_file_drafts(spaces: &ToolWorkspaces, files: Vec<RunFileDraft>) -> Vec<RunFileDraft> {
    #[cfg(windows)]
    fn plain_windows(path: &str) -> String {
        let slash = path.replace('\\',"/");
        if let Some(tail) = slash.strip_prefix("//?/UNC/") { format!("//{tail}") }
        else if let Some(tail) = slash.strip_prefix("//?/") { tail.into() }
        else { slash }
    }
    let mut seen = HashSet::new();
    files.into_iter().filter_map(|mut file| {
        let root = match file.space.as_str() { "user" => &spaces.user_root, "ai" => &spaces.ai_root, _ => return None };
        let path = Path::new(&file.path);
        if path.is_absolute() {
            #[cfg(windows)]
            {
                let candidate = plain_windows(&file.path);
                let base = plain_windows(&root.to_string_lossy());
                let prefix = format!("{}/",base.trim_end_matches('/'));
                if !candidate.to_ascii_lowercase().starts_with(&prefix.to_ascii_lowercase()) { return None; }
                file.path = candidate[prefix.len()..].into();
            }
            #[cfg(not(windows))]
            { file.path = path.strip_prefix(root).ok()?.to_string_lossy().into_owned(); }
        }
        file.path = file.path.replace('\\',"/");
        if file.path.is_empty() || file.path.contains(':') || file.path.starts_with('/')
            || file.path.split('/').any(|part| part.is_empty() || part == "." || part == "..") { return None; }
        let _ = spaces.checked_path(&file.space,&file.path,true);
        if seen.insert((file.space.clone(),file.path.clone(),file.role.clone())) { Some(file) } else { None }
    }).collect()
}

fn record_warning(value: &mut Value, warning: impl Into<String>) {
    if let Some(object) = value.as_object_mut() { object.insert("record_warning".into(), Value::String(warning.into())); }
}

fn attach_workflow_source(mut report: Value, source: Value) -> Value {
    report["source"] = source.clone();
    if serde_json::to_vec(&report).is_ok_and(|bytes| bytes.len() <= 1024 * 1024) {
        return report;
    }
    json!({
        "schema_version":1,
        "state":"limited",
        "outputs":{},
        "step_results":{},
        "trace":[],
        "steps_executed":report.get("steps_executed").cloned().unwrap_or(json!(0)),
        "tool_calls":report.get("tool_calls").cloned().unwrap_or(json!(0)),
        "graph_runs":report.get("graph_runs").cloned().unwrap_or(json!(0)),
        "run_id":report.get("run_id").cloned().unwrap_or(Value::Null),
        "error":{"code":"result_limit","message":"Workflow report exceeds 1 MiB with source metadata; step results and trace cannot be included","step_path":""},
        "source":source
    })
}

async fn request(manager: Arc<BackendManager>, session: String, body: Value) -> Result<Value,String> {
    tauri::async_runtime::spawn_blocking(move || manager.request(&session, body))
        .await.map_err(|_| "Backend request task failed".to_owned())?
}

pub(crate) struct ToolContext {
    pub spaces: ToolWorkspaces,
    pub shared: Arc<BackendManager>,
    pub shared_session: String,
    owned: Option<(Arc<BackendManager>,String)>,
    cleanup_failure: Option<Arc<Mutex<Option<(Arc<BackendManager>,String)>>>>,
    record: Option<(Arc<RunStore>,PathBuf)>,
    workflow_parent: Option<String>,
    workflow_step: Option<String>,
    workflow_outputs: Vec<RunFileDraft>,
    origin: &'static str,
}

impl Drop for ToolContext {
    fn drop(&mut self) {
        // A Tauri command future can be dropped while a blocking RPC still owns another Arc.
        // Keep the owned manager here until its process exit is confirmed; the turn lease drops
        // only after this destructor returns.
        if let Some((manager,_)) = self.owned.take() {
            if let Err(error) = manager.shutdown() {
                if let Some(sink) = &self.cleanup_failure { *sink.lock().unwrap() = Some((manager,error)); }
            }
        }
    }
}

impl ToolContext {
    pub fn new(spaces: ToolWorkspaces, shared: Arc<BackendManager>, shared_session: String) -> Self {
        Self { spaces, shared, shared_session, owned: None, cleanup_failure: None, record: None, workflow_parent: None, workflow_step: None, workflow_outputs: Vec::new(), origin: "ai" }
    }

    pub fn with_manual_origin(mut self) -> Self {
        self.origin = "manual";
        self
    }

    pub fn with_records(mut self, store: Arc<RunStore>, app_data: PathBuf) -> Self {
        self.record = Some((store,app_data));
        self
    }

    pub fn with_failure_sink(mut self, sink: Arc<Mutex<Option<(Arc<BackendManager>,String)>>>) -> Self {
        self.cleanup_failure = Some(sink);
        self
    }

    pub fn sanitize_model_value(&self, value: &mut Value) {
        match value {
            Value::String(text) => {
                for (root,label) in [(&self.spaces.user_root,"user:"),(&self.spaces.ai_root,"ai:")] {
                    let raw = root.to_string_lossy();
                    *text = text.replace(raw.as_ref(),label);
                    *text = text.replace(&raw.replace('\\',"/"),label);
                }
            }
            Value::Array(items) => for item in items { self.sanitize_model_value(item); },
            Value::Object(items) => for item in items.values_mut() { self.sanitize_model_value(item); },
            _ => {}
        }
    }

    async fn ai_backend(&mut self) -> Result<(Arc<BackendManager>,String),String> {
        if let Some(pair) = &self.owned {
            if pair.1.is_empty() { return Err("AI backend connection is still starting".into()); }
            return Ok(pair.clone());
        }
        let manager = Arc::new(self.shared.related_manager());
        let path = self.spaces.ai_root.to_string_lossy().into_owned();
        let connect_manager = manager.clone();
        // Register ownership before the blocking spawn. If this future is dropped while
        // connect runs, Drop waits for the lifecycle lock and then closes that exact process.
        self.owned = Some((manager.clone(),String::new()));
        let info = tauri::async_runtime::spawn_blocking(move ||
            connect_manager.connect(path, false, false, Arc::new(|_| {})))
            .await.map_err(|_| "AI backend connection task failed".to_owned())??;
        let pair = (manager, info.session_id);
        self.owned = Some(pair.clone());
        Ok(pair)
    }

    pub async fn shutdown(&mut self) -> Result<(),String> {
        if let Some((manager,_)) = self.owned.as_ref() {
            let closing = manager.clone();
            let result = tauri::async_runtime::spawn_blocking(move || closing.shutdown().map(|_| ()))
                .await.map_err(|_| "AI backend cleanup task failed".to_owned())?;
            if result.is_ok() { self.owned.take(); }
            result
        } else { Ok(()) }
    }

    async fn graph_args(&mut self, name: &str, args: &Value) -> Result<(Value,String,Value,String,Vec<RunFileDraft>,Value),String> {
        if name == "graph_validate" { fields(args, &["space","graph","mode"], &["options"])?; }
        else { fields(args, &["graph","mode"], &["options"])?; }
        let space = if name == "graph_validate" { arg(args,"space")? } else { "ai" };
        if !matches!(space,"user" | "ai") { return Err("Graph space must be user or ai".into()); }
        let mode = arg(args,"mode")?;
        if !matches!(mode,"offline" | "streaming") { return Err("Graph mode must be offline or streaming".into()); }
        let graph = args.get("graph").filter(|g| g.is_object()).ok_or("Graph must be an object")?.clone();
        let options = args.get("options").cloned().unwrap_or_else(|| json!({}));
        let opts = options.as_object().ok_or("Graph options must be an object")?;
        if (mode == "offline" && !opts.is_empty()) || opts.iter().any(|(key,value)|
            key != "block_frames" || !value.as_u64().is_some_and(|n| (1..=65536).contains(&n))) {
            return Err("Unsupported graph options".into());
        }
        let nodes = graph.get("nodes").and_then(Value::as_array).ok_or("Graph nodes must be an array")?;
        if nodes.is_empty() || nodes.len() > 128 { return Err("Graph must have 1 to 128 nodes".into()); }
        let catalog = ensure_success(request(self.shared.clone(), self.shared_session.clone(), json!({"op":"nodes.list"})).await?)?;
        let definitions = catalog.get("nodes").and_then(Value::as_array).ok_or("Invalid node catalog")?;
        let types: HashMap<&str,&Value> = definitions.iter().filter_map(|item| item.get("typeId").and_then(Value::as_str).map(|id| (id,item))).collect();
        let wanted_domain = if mode == "offline" { "synchronous" } else { "streaming" };
        for node in nodes {
            let kind = node.get("type").and_then(Value::as_str).ok_or("Graph node lacks type")?;
            let definition = types.get(kind).ok_or("Graph contains an unknown node type")?;
            if definition.get("execution_domain").and_then(Value::as_str) != Some(wanted_domain) {
                return Err("Graph mixes execution domains or uses a device node".into());
            }
            let params = node.get("parameters").and_then(Value::as_object);
            let descriptors = definition.get("parameters").and_then(Value::as_array).ok_or("Invalid node parameters catalog")?;
            let file_keys: HashSet<&str> = descriptors.iter().filter(|p| p.get("type").and_then(Value::as_str) == Some("file_path"))
                .filter_map(|p| p.get("id").and_then(Value::as_str)).collect();
            if let Some(params) = params {
                for (key,value) in params {
                    if file_keys.contains(key.as_str()) {
                        let path = value.as_str().ok_or("Graph FilePath must be a string")?;
                        // Both input and output paths are checked. The C++ node decides whether a file must exist.
                        if path.is_empty() { return Err("Graph FilePath cannot be empty".into()); }
                        self.spaces.checked_path(space, path, true)?;
                    }
                }
            }
        }
        let files = normalize_file_drafts(&self.spaces,graph_file_refs(&graph,&catalog,space));
        let plugins = graph_plugin_refs(&graph,&catalog);
        Ok((graph, mode.to_owned(), options, space.to_owned(), files, plugins))
    }

    pub async fn dispatch(&mut self, mode: &str, name: &str, args: &Value, cancel: &AtomicBool) -> Result<Value,String> {
        if !valid_mode(mode) || !names(mode).iter().any(|allowed| allowed == name) { return Err("Tool is not available in this mode".into()); }
        if cancel.load(Ordering::Acquire) { return Err("Agent turn cancelled".into()); }
        if run_history_tools::NAMES.contains(&name) {
            let (store, app_data) = self.record.clone().ok_or("Run history is unavailable for this session")?;
            let spaces = self.spaces.clone();
            let name = name.to_owned();
            let args = args.clone();
            // Await bounded reads/hashing even on cancellation; never detach file-check work.
            let outcome = tauri::async_runtime::spawn_blocking(move ||
                run_history_tools::dispatch(&store, &spaces, &app_data, &name, &args))
                .await.map_err(|_| "Run history query failed".to_owned())?;
            if cancel.load(Ordering::Acquire) { return Err("Agent turn cancelled".into()); }
            return outcome;
        }
        if matches!(name, "workflow_validate" | "workflow_run") {
            return self.dispatch_workflow(name,args,cancel).await;
        }
        self.dispatch_basic(mode,name,args,cancel,None).await
    }

    fn load_workflow(&self, args: &Value, run: bool) -> Result<(workflow::Workflow,Value,Value),String> {
        let optional: &[&str] = if run { &["inputs"] } else { &[] };
        fields(args,&["space","path"],optional)?;
        if let Some(inputs) = args.get("inputs") { if !inputs.is_object() { return Err("Workflow inputs must be an object".into()); } }
        let space = arg(args,"space")?;
        let path = arg(args,"path")?;
        let file = self.spaces.dispatch("file_read_text",&json!({"space":space,"path":path}))?;
        let content = file.get("content").and_then(Value::as_str).ok_or("Workflow file is not UTF-8 text")?;
        let source = json!({"space":space,"path":path,"sha256":format!("{:x}",Sha256::digest(content.as_bytes()))});
        let document = crate::graph_files::strict_json(content.as_bytes())
            .map_err(|error| format!("Workflow JSON is invalid or contains duplicate keys: {error}"))?;
        let program = workflow::validate(&document).map_err(|error| format!("Workflow validation failed: {error}"))?;
        Ok((program,source,document))
    }

    async fn dispatch_workflow(&mut self, name: &str, args: &Value, cancel: &AtomicBool) -> Result<Value,String> {
        let (program,source,document) = self.load_workflow(args,name == "workflow_run")?;
        if name == "workflow_validate" {
            let mut report = workflow::validation_report(&program);
            report.as_object_mut().ok_or("Workflow validation report must be an object")?.insert("source".into(),source);
            return Ok(report);
        }
        self.execute_workflow_snapshot(program,document,source,arg(args,"path")?,args.get("inputs"),
            vec![RunFileDraft { space:arg(args,"space")?.into(),path:arg(args,"path")?.into(),role:"input".into() }],cancel).await
    }

    pub(crate) async fn execute_workflow_snapshot(&mut self, program: workflow::Workflow,
        document: Value, source: Value, name: &str, inputs: Option<&Value>,
        files: Vec<RunFileDraft>, cancel: &AtomicBool) -> Result<Value,String> {
        let record = self.record.as_ref().map(|(store,app_data)| {
            store.begin(&self.spaces,app_data,RunDraft { kind:"workflow".into(),origin:self.origin.into(),
                parent_id:None,name:name.into(),
                configuration:json!({"workflow":document,"inputs_override":inputs.cloned().unwrap_or_else(||json!({})),"source":source,"file_space":"ai"}),
                files })
        });
        let mut warning = None;
        let record_id = match record { Some(Ok(record)) => Some(record.id), Some(Err(error)) => return Err(format!("Cannot begin workflow record; workflow was not started: {error}")), None => None };
        self.workflow_parent = record_id.clone();
        self.workflow_outputs.clear();
        let mut report = serde_json::to_value(workflow::run(&program,inputs,self,cancel).await)
            .unwrap_or_else(|error| json!({"state":"failed","error":{"code":"report_encoding","message":error.to_string()}}));
        // A workflow may have started the private audio backend. Confirm process exit before
        // handing the report back, including cancellation and failed step paths.
        if let Err(error) = self.shutdown().await {
            report["state"] = json!("failed");
            report["outputs"] = json!({});
            report["error"] = json!({"code":"backend_cleanup_failed","message":error,"step_path":null});
        }
        self.workflow_parent = None;
        if let Some(id) = &record_id { report["run_id"] = json!(id); }
        let mut report = attach_workflow_source(report,source);
        if let (Some((store,app_data)),Some(id)) = (&self.record,&record_id) {
            let state = report.get("state").and_then(Value::as_str).unwrap_or("unknown");
            let error = report.get("error").filter(|value| !value.is_null()).map(Value::to_string);
            let mut outputs = std::mem::take(&mut self.workflow_outputs);
            outputs.extend(typed_result_files(&report,"ai"));
            let outputs = normalize_file_drafts(&self.spaces,outputs);
            if let Err(err) = store.finish(&self.spaces,app_data,id,state,Some(report.clone()),error,outputs) {
                warning = Some(format!("Cannot finish workflow record: {err}"));
            }
        }
        if let Some(warning) = warning { record_warning(&mut report,warning); }
        Ok(report)
    }

    async fn dispatch_basic(&mut self, mode: &str, name: &str, args: &Value, cancel: &AtomicBool, workflow_deadline: Option<Instant>) -> Result<Value,String> {
        if !valid_mode(mode) || !names(mode).iter().any(|allowed| allowed == name)
            || matches!(name, "workflow_validate" | "workflow_run")
            || run_history_tools::NAMES.contains(&name) { return Err("Tool is not available in this mode".into()); }
        if cancel.load(Ordering::Acquire) { return Err("Agent turn cancelled".into()); }
        if workflow_deadline.is_some_and(|deadline| Instant::now() >= deadline) { return Err("Workflow deadline exceeded".into()); }
        match name {
            "workspace_list" => { fields(args,&["space"],&["path","offset","limit"])?; self.spaces.dispatch(name,args) }
            "file_read_text" | "audio_inspect" => {
                fields(args,&["space","path"],&[])?;
                if name == "file_read_text" { return self.spaces.dispatch(name,args); }
                let space = arg(args,"space")?;
                let path = arg(args,"path")?;
                self.spaces.checked_path(space,path,false)?;
                let (manager,session) = if space == "user" { (self.shared.clone(),self.shared_session.clone()) }
                    else { self.ai_backend().await? };
                if cancel.load(Ordering::Acquire) { return Err("Agent turn cancelled".into()); }
                ensure_success(request(manager,session,json!({"op":"audio.inspect","path":path})).await?)
            }
            "directory_create" => { fields(args,&["path"],&[])?; self.spaces.dispatch(name,args) }
            "file_write_text" => { fields(args,&["path","content"],&[])?; self.spaces.dispatch(name,args) }
            "file_delete" => { fields(args,&["path"],&[])?; self.spaces.dispatch(name,args) }
            "file_copy_to_ai" => { fields(args,&["source_space","source_path","path"],&[])?; self.spaces.dispatch(name,args) }
            "file_export" => { fields(args,&["path","user_path"],&[])?; self.spaces.dispatch(name,args) }
            "nodes_list" => {
                fields(args,&[],&[])?;
                let response = request(self.shared.clone(),self.shared_session.clone(),json!({"op":"nodes.list"})).await?;
                Ok(json!({"nodes":crate::ai::catalog_nodes(&response)?}))
            }
            "graph_validate" | "graph_run" => {
                let deadline = workflow_deadline.map_or_else(|| Instant::now() + GRAPH_TIMEOUT,
                    |workflow_limit| (Instant::now() + GRAPH_TIMEOUT).min(workflow_limit));
                let mut record_id = None;
                let mut record_outputs = Vec::new();
                let mut record_result = None;
                let mut record_warning_text = None;
                let outcome: Result<Value,String> = async {
                let (graph, execution_mode, options, space, files, plugins) = self.graph_args(name,args).await?;
                if Instant::now() >= deadline { return Err("Graph task exceeded its deadline".into()); }
                if space == "ai" { self.spaces.check_quota()?; }
                if cancel.load(Ordering::Acquire) { return Err("Agent turn cancelled".into()); }
                let (manager,session) = if space == "user" { (self.shared.clone(),self.shared_session.clone()) }
                    else { self.ai_backend().await? };
                if cancel.load(Ordering::Acquire) { return Err("Agent turn cancelled".into()); }
                if name == "graph_run" {
                    let (outputs,inputs): (Vec<_>,Vec<_>) = files.into_iter().partition(|file| file.role == "output");
                    record_outputs = outputs;
                    if let Some((store,app_data)) = &self.record {
                        let mut configuration = json!({"mode":execution_mode,"graph":graph,"options":options,"workflow_step_path":self.workflow_step,"file_space":space});
                        if plugins.as_array().is_some_and(|values| !values.is_empty()) { configuration["node_plugins"] = plugins; }
                        match store.begin(&self.spaces,app_data,RunDraft { kind:"graph".into(),origin:self.origin.into(),
                            parent_id:self.workflow_parent.clone(),name:self.workflow_step.clone().unwrap_or_else(||"Graph run".into()),
                            configuration,files:inputs }) {
                            Ok(record) => record_id = Some(record.id),
                            Err(error) => return Err(format!("Cannot begin graph record; graph was not started: {error}")),
                        }
                    }
                }
                let operation = if name == "graph_validate" { "graph.validate" } else { "tasks.start" };
                let start = ensure_success(request(manager.clone(),session.clone(),json!({"op":operation,"mode":execution_mode,"graph":graph,"options":options})).await?)?;
                if name == "graph_validate" { return Ok(start); }
                let task_id = start.get("task_id").and_then(Value::as_str).ok_or("Task start lacks task_id")?.to_owned();
                loop {
                    if let Err(error) = self.spaces.check_quota() {
                        let _ = request(manager.clone(),session.clone(),json!({"op":"tasks.cancel","task_id":task_id})).await;
                        return Err(format!("Graph exceeded AI workspace quota: {error}"));
                    }
                    if cancel.load(Ordering::Acquire) || Instant::now() >= deadline {
                        let _ = request(manager.clone(),session.clone(),json!({"op":"tasks.cancel","task_id":task_id})).await;
                        return Err(if cancel.load(Ordering::Acquire) { "Graph task cancelled" } else { "Graph task exceeded its deadline" }.into());
                    }
                    let status = ensure_success(request(manager.clone(),session.clone(),json!({"op":"tasks.status","task_id":task_id})).await?)?;
                    match status.get("state").and_then(Value::as_str) {
                        Some("succeeded") => {
                            let outcome = ensure_success(request(manager.clone(),session.clone(),json!({"op":"tasks.result","task_id":task_id})).await?)?;
                            record_result = Some(outcome.clone());
                            if outcome.get("state").and_then(Value::as_str) != Some("succeeded") {
                                return Err(format!("Graph task ended without success: {outcome}"));
                            }
                            let _ = request(manager,session,json!({"op":"tasks.release","task_id":task_id})).await;
                            self.spaces.check_quota()?;
                            return Ok(outcome);
                        }
                        Some("failed" | "cancelled") => {
                            let outcome = request(manager.clone(),session.clone(),json!({"op":"tasks.result","task_id":task_id})).await.unwrap_or(status);
                            record_result = Some(outcome.clone());
                            return Err(format!("Graph task failed or was cancelled: {outcome}"));
                        }
                        Some("queued" | "running" | "cancelling") => tokio::time::sleep(Duration::from_millis(100)).await,
                        _ => return Err(format!("Unknown Graph task state: {status}")),
                    }
                }
                }.await;
                let mut outcome = if name == "graph_run" && workflow_deadline.is_none() {
                    let cleanup = self.shutdown().await;
                    match (outcome,cleanup) {
                        (Ok(value),Ok(())) => Ok(value),
                        (Err(error),Ok(())) => Err(error),
                        (Ok(_),Err(error)) => Err(format!("Graph backend cleanup failed: {error}")),
                        (Err(original),Err(cleanup)) => Err(format!("{original}; Graph backend cleanup failed: {cleanup}")),
                    }
                } else { outcome };
                if name == "graph_run" {
                    if let (Some((store,app_data)),Some(id)) = (&self.record,&record_id) {
                        let state = record_result.as_ref().and_then(|v| v.get("state").or_else(||v.pointer("/data/state")).and_then(Value::as_str))
                            .unwrap_or(if cancel.load(Ordering::Acquire) { "cancelled" } else if outcome.is_ok() { "succeeded" } else { "failed" });
                        let value = record_result.clone().or_else(|| outcome.as_ref().ok().cloned());
                        let error = outcome.as_ref().err().cloned();
                        let mut outputs = record_outputs;
                        if let Some(value) = &value { outputs.extend(typed_result_files(value,"ai")); }
                        let outputs = normalize_file_drafts(&self.spaces,outputs);
                        if let Err(error) = store.finish(&self.spaces,app_data,id,state,value,error,outputs) {
                            record_warning_text = Some(format!("Cannot finish graph record: {error}"));
                        }
                        match &mut outcome {
                            Ok(value) => { value["run_id"] = json!(id); },
                            Err(error) => error.push_str(&format!("; run_id={id}")),
                        }
                    }
                    if let Some(warning) = record_warning_text {
                        match &mut outcome { Ok(value) => record_warning(value,warning), Err(error) => error.push_str(&format!("; run record warning: {warning}")) }
                    }
                }
                outcome
            }
            _ => Err("Unknown tool".into()),
        }
    }
}

impl WorkflowHost for ToolContext {
    fn call<'a>(&'a mut self, tool: &'a str, args: Value, cancel: &'a AtomicBool, deadline: Instant, step_path: &'a str)
        -> Pin<Box<dyn Future<Output = Result<Value,String>> + Send + 'a>> {
        Box::pin(async move {
            if matches!(tool, "workflow_validate" | "workflow_run") {
                return Err("Nested workflow calls are not allowed".into());
            }
            self.workflow_step = Some(step_path.into());
            let result = self.dispatch_basic("workflow",tool,&args,cancel,Some(deadline)).await;
            if let Ok(value) = &result {
                if matches!(tool,"file_export" | "file_write_text" | "file_copy_to_ai") {
                    if let (Some(space),Some(path)) = (value.get("space").and_then(Value::as_str),value.get("path").and_then(Value::as_str)) {
                        self.workflow_outputs.push(RunFileDraft { space:space.into(),path:path.into(),role:"output".into() });
                    }
                }
            }
            self.workflow_step = None;
            result
        })
    }
}

#[cfg(test)]
#[path = "agent_tools_tests.rs"]
mod tests;
