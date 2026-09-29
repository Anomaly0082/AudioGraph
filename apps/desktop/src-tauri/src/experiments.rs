use crate::backend::BackendManager;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, atomic::{AtomicU64, Ordering}};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_RECORD: u64 = 4 * 1024 * 1024;
const MAX_INPUT: u64 = 256 * 1024 * 1024;
const MAX_ROUNDS: usize = 20;
const MAX_GOAL: usize = 16 * 1024;

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExperimentParameter {
    pub node_id: String, pub parameter_id: String, pub minimum: f64, pub maximum: f64,
    pub integer_only: bool,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExperimentSpec { pub goal: String, pub base: Value, pub parameters: Vec<ExperimentParameter> }

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExperimentInput { pub node_id: String, pub original_path: String, pub snapshot_path: String }

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateFeedback { pub rating: String, pub note: String }

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentCandidate {
    pub id: String, pub label: String, pub values: Vec<f64>, pub state: String,
    pub output_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback: Option<CandidateFeedback>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentRound { pub id: String, pub candidates: Vec<ExperimentCandidate> }

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentRecord {
    pub schema_version: u32, pub id: String, pub workspace: String, pub created_at: u64,
    pub input: ExperimentInput, pub output_node_id: String,
    #[serde(flatten)] pub spec: ExperimentSpec,
    pub rounds: Vec<ExperimentRound>,
}

#[derive(Serialize)]
pub struct ExperimentSummary { pub id: String, pub goal: String, pub created_at: u64 }

#[derive(Default)]
pub struct ExperimentStore { lock: Mutex<()>, next_id: AtomicU64 }

fn now_ms() -> Result<u64, String> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| "系统时间无效")?.as_millis() as u64)
}

fn safe_id(id: &str) -> bool {
    id.starts_with("ex") && id.len() <= 48 && id[2..].split_once('-').is_some_and(|(time, serial)|
        !time.is_empty() && !serial.is_empty() && time.bytes().all(|b| b.is_ascii_digit()) && serial.bytes().all(|b| b.is_ascii_digit()))
}

fn no_reparse(path: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("无法检查实验路径：{e}"))?;
    if meta.file_type().is_symlink() { return Err("实验路径不能包含符号链接".into()); }
    #[cfg(windows)] {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 { return Err("实验路径不能包含reparse point或junction".into()); }
    }
    Ok(())
}

fn check_path_chain(workspace: &Path, path: &Path) -> Result<(), String> {
    let relative = path.strip_prefix(workspace).map_err(|_| "文件必须在当前工作区内")?;
    let mut current = workspace.to_path_buf();
    no_reparse(&current)?;
    for part in relative.components() {
        if !matches!(part, Component::Normal(_)) { return Err("文件路径无效".into()); }
        current.push(part);
        no_reparse(&current)?;
    }
    Ok(())
}

fn existing_source(workspace: &Path, requested: &str) -> Result<PathBuf, String> {
    if requested.is_empty() || requested.contains('\0') { return Err("输入路径无效".into()); }
    let input = Path::new(requested);
    let joined = if input.is_absolute() { input.to_path_buf() } else { workspace.join(input) };
    let path = fs::canonicalize(&joined).map_err(|e| format!("无法定位输入WAV：{e}"))?;
    if !path.starts_with(workspace) { return Err("输入WAV必须在当前工作区内".into()); }
    // Check the spelling supplied by the user too: canonicalization alone accepts links back into the workspace.
    let mut current = if input.is_absolute() { PathBuf::new() } else { workspace.to_path_buf() };
    for part in input.components() {
        match part {
            Component::Prefix(prefix) if input.is_absolute() => current.push(prefix.as_os_str()),
            Component::RootDir if input.is_absolute() => current.push(part.as_os_str()),
            Component::Normal(name) => { current.push(name); if current.is_absolute() { no_reparse(&current)?; } },
            Component::CurDir => continue,
            _ => return Err("输入路径不能包含上级目录或重解析路径".into()),
        }
    }
    check_path_chain(workspace, &path)?;
    let meta = fs::metadata(&path).map_err(|e| format!("无法读取输入WAV属性：{e}"))?;
    if !meta.is_file() || meta.len() > MAX_INPUT { return Err("输入必须是普通WAV文件且不超过256MiB".into()); }
    if !path.extension().and_then(|s| s.to_str()).is_some_and(|s| s.eq_ignore_ascii_case("wav")) {
        return Err("输入必须是WAV文件".into());
    }
    Ok(path)
}

fn root(workspace: &Path, create: bool) -> Result<PathBuf, String> {
    no_reparse(workspace)?;
    let path = workspace.join(".audio-experiments");
    if create { fs::create_dir(&path).or_else(|e| if e.kind() == std::io::ErrorKind::AlreadyExists { Ok(()) } else { Err(e) })
        .map_err(|e| format!("无法创建实验目录：{e}"))?; }
    no_reparse(&path)?;
    if !path.is_dir() { return Err("实验目录不是普通目录".into()); }
    Ok(path)
}

fn record_path(workspace: &Path, id: &str) -> Result<PathBuf, String> {
    if !safe_id(id) { return Err("实验ID无效".into()); }
    let folder = root(workspace, false)?.join(id);
    no_reparse(&folder)?;
    if !folder.is_dir() { return Err("实验目录无效".into()); }
    let path = folder.join("record.json");
    no_reparse(&path)?;
    if !path.is_file() { return Err("实验记录不是普通文件".into()); }
    Ok(path)
}

fn digest_file(path: &Path) -> Result<String, String> {
    no_reparse(path)?;
    let metadata = fs::metadata(path).map_err(|e| format!("无法读取输入快照：{e}"))?;
    if !metadata.is_file() || metadata.len() > MAX_INPUT { return Err("输入快照不是普通文件或超过256MiB".into()); }
    let mut file = File::open(path).map_err(|e| format!("无法打开输入快照：{e}"))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let count = file.read(&mut buffer).map_err(|e| format!("读取输入快照失败：{e}"))?;
        if count == 0 { break; }
        total += count as u64;
        if total > MAX_INPUT { return Err("输入快照超过256MiB".into()); }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn verify_snapshot(workspace: &Path, record: &ExperimentRecord) -> Result<(), String> {
    let snapshot = workspace.join(&record.input.snapshot_path);
    check_path_chain(workspace, &snapshot)?;
    let manifest = snapshot.parent().ok_or("输入快照目录无效")?.join("input.sha256");
    check_path_chain(workspace, &manifest)?;
    let mut expected = String::new();
    File::open(manifest).map_err(|e| format!("无法读取输入快照校验值：{e}"))?
        .take(65).read_to_string(&mut expected).map_err(|e| format!("输入校验值格式无效：{e}"))?;
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) ||
        digest_file(&snapshot)? != expected {
        return Err("输入快照内容已改变，不能继续实验".into());
    }
    Ok(())
}

fn bounded_json(value: &Value, max: usize, name: &str) -> Result<(), String> {
    if serde_json::to_vec(value).map_err(|_| format!("{name}无法编码"))?.len() > max {
        return Err(format!("{name}超过大小限制"));
    }
    Ok(())
}

fn no_secret_keys(value: &Value) -> Result<(), String> {
    match value {
        Value::Object(map) => for (key, value) in map {
            let normalized = key.to_ascii_lowercase().replace(['_', '-'], "");
            if ["apikey", "authorization", "accesstoken", "refreshtoken", "password", "secret"].contains(&normalized.as_str()) {
                return Err("实验记录不能包含凭据字段".into());
            }
            no_secret_keys(value)?;
        },
        Value::Array(items) => for value in items { no_secret_keys(value)?; },
        _ => {}
    }
    Ok(())
}

fn catalog_map(catalog: &Value) -> Result<&[Value], String> {
    if catalog.get("success") != Some(&Value::Bool(true)) { return Err("读取节点目录失败".into()); }
    catalog.pointer("/data/nodes").and_then(Value::as_array).map(Vec::as_slice).ok_or("节点目录格式无效".into())
}

fn graph_parts(base: &Value) -> Result<(&Value, &[Value]), String> {
    if base.get("mode").and_then(Value::as_str) != Some("offline") ||
        !base.get("options").and_then(Value::as_object).is_some_and(|v| v.is_empty()) {
        return Err("实验只支持不带执行选项的offline Graph".into());
    }
    let graph = base.get("graph").ok_or("Graph缺失")?;
    let nodes = graph.get("nodes").and_then(Value::as_array).ok_or("Graph节点无效")?;
    if nodes.is_empty() || nodes.len() > 128 { return Err("Graph节点数量无效".into()); }
    Ok((graph, nodes))
}

fn validate_spec(spec: &ExperimentSpec, catalog: &[Value]) -> Result<(String, String, String), String> {
    if spec.goal.trim().is_empty() || spec.goal.len() > MAX_GOAL { return Err("实验目标不能为空且不超过16KiB".into()); }
    if !(1..=4).contains(&spec.parameters.len()) { return Err("实验必须声明1～4个可调参数".into()); }
    bounded_json(&spec.base, 256 * 1024, "基线Graph")?;
    let (_, nodes) = graph_parts(&spec.base)?;
    let mut input = None;
    let mut output = None;
    let mut ids = HashSet::new();
    for node in nodes {
        let id = node.get("id").and_then(Value::as_str).ok_or("Graph节点ID无效")?;
        let kind = node.get("type").and_then(Value::as_str).ok_or("Graph节点类型无效")?;
        if !ids.insert(id) { return Err("Graph节点ID重复".into()); }
        let descriptor = catalog.iter().find(|n| n.get("typeId").and_then(Value::as_str) == Some(kind) &&
            n.get("execution_domain").and_then(Value::as_str) == Some("synchronous"))
            .ok_or("Graph含未注册或非离线节点")?;
        if kind == "wav_input" {
            if input.is_some() { return Err("实验只允许一个wav_input".into()); }
            let path = node.pointer("/parameters/path").and_then(Value::as_str).ok_or("wav_input缺少路径")?;
            input = Some((id.to_owned(), path.to_owned()));
        }
        if kind == "wav_output" {
            if output.is_some() { return Err("实验只允许一个wav_output".into()); }
            output = Some(id.to_owned());
        }
        let schema = descriptor.get("parameters").and_then(Value::as_array).ok_or("节点参数目录无效")?;
        for item in schema {
            if item.get("type").and_then(Value::as_str) == Some("file_path") {
                let field = item.get("id").and_then(Value::as_str).ok_or("文件参数ID无效")?;
                if !((kind == "wav_input" || kind == "wav_output") && field == "path") {
                    return Err("实验Graph不允许其他FilePath参数".into());
                }
            }
        }
    }
    let (input_id, input_path) = input.ok_or("实验必须恰好包含一个wav_input")?;
    let output_id = output.ok_or("实验必须恰好包含一个wav_output")?;
    let mut parameter_keys = HashSet::new();
    for parameter in &spec.parameters {
        if !parameter.minimum.is_finite() || !parameter.maximum.is_finite() || parameter.minimum > parameter.maximum {
            return Err("参数范围必须是有限且有序的数值".into());
        }
        if !parameter_keys.insert((&parameter.node_id, &parameter.parameter_id)) { return Err("重复的可调参数".into()); }
        let node = nodes.iter().find(|n| n.get("id").and_then(Value::as_str) == Some(&parameter.node_id))
            .ok_or("可调参数节点不存在")?;
        let kind = node.get("type").and_then(Value::as_str).ok_or("节点类型无效")?;
        let schema = catalog.iter().find(|n| n.get("typeId").and_then(Value::as_str) == Some(kind))
            .and_then(|n| n.get("parameters")).and_then(Value::as_array).ok_or("节点参数目录无效")?;
        let definition = schema.iter().find(|p| p.get("id").and_then(Value::as_str) == Some(&parameter.parameter_id))
            .ok_or("可调参数不在节点Schema中")?;
        if definition.get("type").and_then(Value::as_str) != Some("number") { return Err("可调参数必须是数值类型".into()); }
        let integer_only = definition.get("integer_only").and_then(Value::as_bool).unwrap_or(false);
        if parameter.integer_only != integer_only { return Err("integer_only必须与节点Schema一致".into()); }
        if definition.get("minimum").and_then(Value::as_f64).is_some_and(|n| parameter.minimum < n) ||
            definition.get("maximum").and_then(Value::as_f64).is_some_and(|n| parameter.maximum > n) {
            return Err("参数范围超出节点Schema限制".into());
        }
        if integer_only && parameter.minimum.ceil() > parameter.maximum.floor() { return Err("整数参数范围内没有整数".into()); }
    }
    Ok((input_id, input_path, output_id))
}

pub(crate) fn validate_values(parameters: &[ExperimentParameter], values: &[f64]) -> Result<(), String> {
    if values.len() != parameters.len() { return Err("候选值数量与参数定义不符".into()); }
    for (value, parameter) in values.iter().zip(parameters) {
        if !value.is_finite() || *value < parameter.minimum || *value > parameter.maximum ||
            (parameter.integer_only && value.fract() != 0.0) {
            return Err("候选值不符合参数范围或整数规则".into());
        }
    }
    Ok(())
}

fn validate_record(record: &ExperimentRecord, workspace: &Path) -> Result<(), String> {
    if record.schema_version != 1 || !safe_id(&record.id) || record.workspace != workspace.to_string_lossy() {
        return Err("实验版本、ID或工作区不匹配".into());
    }
    if record.spec.goal.trim().is_empty() || record.spec.goal.len() > MAX_GOAL || !(1..=4).contains(&record.spec.parameters.len()) ||
        record.rounds.len() > MAX_ROUNDS { return Err("实验目标、参数或轮次数量无效".into()); }
    let snapshot = format!(".audio-experiments/{}/input.wav", record.id);
    if record.input.snapshot_path != snapshot { return Err("输入快照路径已改变".into()); }
    graph_parts(&record.spec.base)?;
    let nodes = record.spec.base.pointer("/graph/nodes").and_then(Value::as_array).ok_or("Graph节点无效")?;
    if nodes.iter().filter(|n| n.get("type").and_then(Value::as_str) == Some("wav_input")).count() != 1 ||
        nodes.iter().filter(|n| n.get("type").and_then(Value::as_str) == Some("wav_output")).count() != 1 ||
        !nodes.iter().any(|n| n.get("id").and_then(Value::as_str) == Some(&record.input.node_id) &&
            n.pointer("/parameters/path").and_then(Value::as_str) == Some(&snapshot)) ||
        !nodes.iter().any(|n| n.get("id").and_then(Value::as_str) == Some(&record.output_node_id) &&
            n.get("type").and_then(Value::as_str) == Some("wav_output")) {
        return Err("实验基线与固定输入输出不符".into());
    }
    let mut all_ids = HashSet::new();
    for (index, round) in record.rounds.iter().enumerate() {
        if round.id != format!("r{}", index + 1) || !(2..=4).contains(&round.candidates.len()) { return Err("实验轮次必须按顺序包含2～4个候选".into()); }
        for (candidate_index, candidate) in round.candidates.iter().enumerate() {
            if candidate.id != format!("c{}", candidate_index + 1) || !all_ids.insert(format!("{}-{}", round.id, candidate.id)) ||
                candidate.output_path != format!(".audio-experiments/{}/{}-{}.wav", record.id, round.id, candidate.id) {
                return Err("候选ID或输出路径无效".into());
            }
            if candidate.label.trim().is_empty() || candidate.label.len() > 512 ||
                !["planned", "starting", "running", "succeeded", "failed", "cancelled", "interrupted"].contains(&candidate.state.as_str()) {
                return Err("候选标签或状态无效".into());
            }
            validate_values(&record.spec.parameters, &candidate.values)?;
            if candidate.task_id.as_ref().is_some_and(|s| s.is_empty() || s.len() > 128) { return Err("任务ID无效".into()); }
            if candidate.state == "succeeded" && candidate.result.is_none() {
                return Err("成功候选缺少已保存的任务结果".into());
            }
            if let Some(value) = &candidate.result { bounded_json(value, 64 * 1024, "候选结果")?; }
            if let Some(value) = &candidate.errors { bounded_json(value, 64 * 1024, "候选错误")?; }
            if let Some(feedback) = &candidate.feedback {
                if !["preferred", "acceptable", "rejected"].contains(&feedback.rating.as_str()) || feedback.note.len() > 16 * 1024 || candidate.state != "succeeded" {
                    return Err("人工评价只能附在成功候选上且不超过16KiB".into());
                }
            }
        }
    }
    let value = serde_json::to_value(record).map_err(|_| "记录无法编码")?;
    no_secret_keys(&value)?;
    bounded_json(&value, MAX_RECORD as usize, "实验记录")
}

fn read_record(path: &Path, workspace: &Path) -> Result<ExperimentRecord, String> {
    let mut bytes = Vec::new();
    File::open(path).map_err(|e| format!("读取实验记录失败：{e}"))?.take(MAX_RECORD + 1)
        .read_to_end(&mut bytes).map_err(|e| format!("读取实验记录失败：{e}"))?;
    if bytes.len() as u64 > MAX_RECORD { return Err("实验记录超过4MiB".into()); }
    let value = crate::graph_files::strict_json(&bytes)?;
    let record: ExperimentRecord = serde_json::from_value(value).map_err(|e| format!("实验记录损坏：{e}"))?;
    validate_record(&record, workspace)?;
    Ok(record)
}

fn write_record(path: &Path, record: &ExperimentRecord, create: bool) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(record).map_err(|_| "记录无法编码")?;
    if bytes.len() as u64 > MAX_RECORD { return Err("实验记录超过4MiB".into()); }
    let tmp = path.with_extension(format!("tmp-{}", now_ms()?));
    let write_result = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)
            .map_err(|e| format!("无法创建临时记录：{e}"))?;
        file.write_all(&bytes).and_then(|_| file.sync_all()).map_err(|e| format!("无法持久化临时记录：{e}"))?;
        if create && path.exists() { return Err("实验记录已经存在".into()); }
        if !create { no_reparse(path)?; }
        fs::rename(&tmp, path).map_err(|e| format!("无法原子替换实验记录：{e}"))
    })();
    if write_result.is_err() { let _ = fs::remove_file(&tmp); }
    write_result
}

fn backend_ok(backend: &BackendManager, session_id: &str, request: Value, label: &str) -> Result<Value, String> {
    let response = backend.request(session_id, request)?;
    if response.get("success") != Some(&Value::Bool(true)) {
        let detail = response.get("errors").map(Value::to_string).unwrap_or_default();
        return Err(format!("{label}失败：{}", detail.chars().take(2048).collect::<String>()));
    }
    Ok(response)
}

impl ExperimentStore {
    pub fn create(&self, backend: &BackendManager, session_id: &str, mut spec: ExperimentSpec) -> Result<ExperimentRecord, String> {
        let _guard = self.lock.lock().unwrap();
        let workspace = backend.workspace(session_id)?;
        let catalog = backend_ok(backend, session_id, json!({"op":"nodes.list"}), "节点目录")?;
        let (input_id, input_original, output_id) = validate_spec(&spec, catalog_map(&catalog)?)?;
        let source = existing_source(&workspace, &input_original)?;
        backend_ok(backend, session_id, json!({"op":"audio.inspect","path":input_original}), "输入检查")?;
        backend_ok(backend, session_id, json!({"op":"graph.validate","mode":"offline","graph":spec.base["graph"],"options":{}}), "Graph校验")?;
        let parent = root(&workspace, true)?;
        let mut folder = None;
        let mut id = String::new();
        for _ in 0..8 {
            id = format!("ex{}-{}", now_ms()?, self.next_id.fetch_add(1, Ordering::Relaxed));
            let path = parent.join(&id);
            match fs::create_dir(&path) {
                Ok(()) => { folder = Some(path); break; },
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("无法创建实验目录：{e}")),
            }
        }
        let folder = folder.ok_or("无法生成唯一实验ID")?;
        let snapshot_relative = format!(".audio-experiments/{id}/input.wav");
        let snapshot = folder.join("input.wav");
        let create_result = (|| {
            let mut from = File::open(&source).map_err(|e| format!("无法打开输入WAV：{e}"))?;
            let mut to = OpenOptions::new().write(true).create_new(true).open(&snapshot)
                .map_err(|e| format!("无法创建输入快照：{e}"))?;
            let copied = std::io::copy(&mut Read::by_ref(&mut from).take(MAX_INPUT + 1), &mut to)
                .map_err(|e| format!("复制输入WAV失败：{e}"))?;
            if copied > MAX_INPUT { return Err("输入WAV在复制过程中超过256MiB".into()); }
            to.sync_all().map_err(|e| format!("无法持久化输入快照：{e}"))?;
            let digest = digest_file(&snapshot)?;
            let mut manifest = OpenOptions::new().write(true).create_new(true).open(folder.join("input.sha256"))
                .map_err(|e| format!("无法创建输入校验值：{e}"))?;
            manifest.write_all(digest.as_bytes()).and_then(|_| manifest.sync_all())
                .map_err(|e| format!("无法持久化输入校验值：{e}"))?;
            backend_ok(backend, session_id, json!({"op":"audio.inspect","path":snapshot_relative}), "输入快照检查")?;
            let nodes = spec.base.pointer_mut("/graph/nodes").and_then(Value::as_array_mut).ok_or("Graph节点无效")?;
            let input = nodes.iter_mut().find(|n| n.get("id").and_then(Value::as_str) == Some(&input_id)).ok_or("输入节点已消失")?;
            input["parameters"]["path"] = json!(snapshot_relative);
            backend_ok(backend, session_id, json!({"op":"graph.validate","mode":"offline","graph":spec.base["graph"],"options":{}}), "快照Graph校验")?;
            let record = ExperimentRecord { schema_version: 1, id: id.clone(), workspace: workspace.to_string_lossy().into_owned(),
                created_at: now_ms()?, input: ExperimentInput { node_id: input_id, original_path: input_original,
                    snapshot_path: snapshot_relative }, output_node_id: output_id, spec, rounds: vec![] };
            validate_record(&record, &workspace)?;
            backend.workspace(session_id)?;
            write_record(&folder.join("record.json"), &record, true)?;
            Ok(record)
        })();
        if create_result.is_err() && no_reparse(&folder).is_ok() {
            for name in ["record.json", "input.sha256", "input.wav"] {
                let path = folder.join(name);
                if no_reparse(&path).is_ok() { let _ = fs::remove_file(path); }
            }
            let _ = fs::remove_dir(&folder);
        }
        create_result
    }

    fn list_with_check(&self, workspace: &Path,
        mut validate: impl FnMut(&ExperimentRecord) -> Result<(), String>) -> Result<Vec<ExperimentSummary>, String> {
        let _guard = self.lock.lock().unwrap();
        let parent = workspace.join(".audio-experiments");
        if fs::symlink_metadata(&parent).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) { return Ok(vec![]); }
        let parent = root(workspace, false)?;
        let mut summaries = Vec::new();
        for entry in fs::read_dir(parent).map_err(|e| format!("无法列出实验：{e}"))? {
            let entry = entry.map_err(|e| format!("无法读取实验目录：{e}"))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !safe_id(&name) { continue; }
            let path = record_path(workspace, &name)?;
            let record = read_record(&path, workspace)?;
            if record.id != name { return Err("实验目录与记录ID不一致".into()); }
            verify_snapshot(workspace, &record)?;
            validate(&record)?;
            summaries.push(ExperimentSummary { id: record.id, goal: record.spec.goal, created_at: record.created_at });
        }
        summaries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(summaries)
    }

    #[cfg(test)]
    pub fn list(&self, workspace: &Path) -> Result<Vec<ExperimentSummary>, String> {
        self.list_with_check(workspace, |_| Ok(()))
    }

    fn load_with_check(&self, workspace: &Path, id: &str,
        validate: impl FnOnce(&ExperimentRecord) -> Result<(), String>) -> Result<ExperimentRecord, String> {
        let _guard = self.lock.lock().unwrap();
        let path = record_path(workspace, id)?;
        let record = read_record(&path, workspace)?;
        if record.id != id { return Err("实验目录与记录ID不一致".into()); }
        verify_snapshot(workspace, &record)?;
        validate(&record)?;
        let changed = record.rounds.iter().any(|round| round.candidates.iter().any(|c| c.state == "starting" || c.state == "running"));
        let interrupted = interrupt_active(record);
        if changed {
            // Persist the recovered state so a later save cannot resurrect an unfinished task.
            write_record(&path, &interrupted, false)?;
        }
        Ok(interrupted)
    }

    #[cfg(test)]
    pub fn load(&self, workspace: &Path, id: &str) -> Result<ExperimentRecord, String> {
        self.load_with_check(workspace, id, |_| Ok(()))
    }

    pub fn save(&self, workspace: &Path, record: ExperimentRecord) -> Result<(), String> {
        let _guard = self.lock.lock().unwrap();
        validate_record(&record, workspace)?;
        let path = record_path(workspace, &record.id)?;
        let previous = read_record(&path, workspace)?;
        if record.schema_version != previous.schema_version || record.id != previous.id || record.workspace != previous.workspace ||
            record.created_at != previous.created_at || record.input != previous.input ||
            record.output_node_id != previous.output_node_id || record.spec != previous.spec {
            return Err("实验核心定义不可修改".into());
        }
        if record.rounds.len() < previous.rounds.len() { return Err("不能删除已保存的轮次".into()); }
        for (old_round, new_round) in previous.rounds.iter().zip(&record.rounds) {
            if old_round.candidates.len() != new_round.candidates.len() { return Err("不能更改已保存的候选数量".into()); }
            for (old, new) in old_round.candidates.iter().zip(&new_round.candidates) {
                if old.id != new.id || old.label != new.label || old.values != new.values || old.output_path != new.output_path {
                    return Err("已保存候选的ID、标签、数值和路径不可修改".into());
                }
            }
        }
        for round in record.rounds.iter().skip(previous.rounds.len()) {
            for candidate in &round.candidates {
                let output = workspace.join(&candidate.output_path);
                if fs::symlink_metadata(&output).is_ok() { return Err("新候选输出路径已存在，不能覆盖".into()); }
            }
        }
        verify_snapshot(workspace, &record)?;
        write_record(&path, &record, false)
    }

    fn validate_live(backend: &BackendManager, session_id: &str, record: &ExperimentRecord) -> Result<(), String> {
        let catalog = backend_ok(backend, session_id, json!({"op":"nodes.list"}), "节点目录")?;
        let (input, path, output) = validate_spec(&record.spec, catalog_map(&catalog)?)?;
        if input != record.input.node_id || path != record.input.snapshot_path || output != record.output_node_id {
            return Err("已保存实验的输入输出与节点目录不符".into());
        }
        backend_ok(backend, session_id, json!({"op":"graph.validate","mode":"offline",
            "graph":record.spec.base["graph"],"options":{}}), "已保存Graph校验")?;
        Ok(())
    }

    pub fn load_checked(&self, backend: &BackendManager, session_id: &str, id: &str) -> Result<ExperimentRecord, String> {
        let workspace = backend.workspace(session_id)?;
        let record = self.load_with_check(&workspace, id, |record| Self::validate_live(backend, session_id, record))?;
        backend.workspace(session_id)?;
        Ok(record)
    }

    pub fn peek_checked(&self, backend: &BackendManager, session_id: &str, id: &str) -> Result<ExperimentRecord, String> {
        let workspace = backend.workspace(session_id)?;
        let _guard = self.lock.lock().unwrap();
        let record = read_record(&record_path(&workspace, id)?, &workspace)?;
        if record.id != id { return Err("实验目录与记录ID不一致".into()); }
        verify_snapshot(&workspace, &record)?;
        Self::validate_live(backend, session_id, &record)?;
        backend.workspace(session_id)?;
        Ok(record)
    }

    pub fn list_checked(&self, backend: &BackendManager, session_id: &str) -> Result<Vec<ExperimentSummary>, String> {
        let workspace = backend.workspace(session_id)?;
        let summaries = self.list_with_check(&workspace, |record| Self::validate_live(backend, session_id, record))?;
        backend.workspace(session_id)?;
        Ok(summaries)
    }

    pub fn save_checked(&self, backend: &BackendManager, session_id: &str, record: ExperimentRecord) -> Result<(), String> {
        let workspace = backend.workspace(session_id)?;
        Self::validate_live(backend, session_id, &record)?;
        self.save(&workspace, record)
    }
}

pub(crate) fn interrupt_active(mut record: ExperimentRecord) -> ExperimentRecord {
    for round in &mut record.rounds { for candidate in &mut round.candidates {
        if candidate.state == "starting" || candidate.state == "running" { candidate.state = "interrupted".into(); }
    }}
    record
}

#[tauri::command]
pub async fn experiment_create(store: tauri::State<'_, Arc<ExperimentStore>>, backend: tauri::State<'_, Arc<BackendManager>>,
    session_id: String, spec: ExperimentSpec) -> Result<ExperimentRecord, String> {
    let store = store.inner().clone();
    let backend = backend.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.create(&backend, &session_id, spec))
        .await.map_err(|_| "创建实验任务失败".to_owned())?
}

#[tauri::command]
pub async fn experiment_list(store: tauri::State<'_, Arc<ExperimentStore>>, backend: tauri::State<'_, Arc<BackendManager>>,
    session_id: String) -> Result<Vec<ExperimentSummary>, String> {
    let store = store.inner().clone();
    let backend = backend.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.list_checked(&backend, &session_id))
        .await.map_err(|_| "列出实验任务失败".to_owned())?
}

#[tauri::command]
pub async fn experiment_load(store: tauri::State<'_, Arc<ExperimentStore>>, backend: tauri::State<'_, Arc<BackendManager>>,
    session_id: String, id: String) -> Result<ExperimentRecord, String> {
    let store = store.inner().clone();
    let backend = backend.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.load_checked(&backend, &session_id, &id))
        .await.map_err(|_| "加载实验任务失败".to_owned())?
}

#[tauri::command]
pub async fn experiment_save(store: tauri::State<'_, Arc<ExperimentStore>>, backend: tauri::State<'_, Arc<BackendManager>>,
    session_id: String, record: ExperimentRecord) -> Result<(), String> {
    let store = store.inner().clone();
    let backend = backend.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.save_checked(&backend, &session_id, record))
        .await.map_err(|_| "保存实验任务失败".to_owned())?
}

#[cfg(test)]
#[path = "experiments_tests.rs"]
mod tests;
