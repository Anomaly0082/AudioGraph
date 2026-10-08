//! The deliberately small, deterministic Workflow v1 interpreter.
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::{
    collections::HashSet,
    fmt,
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const MAX_DOCUMENT: usize = 64 * 1024;
const MAX_VALUE: usize = 256 * 1024;
const MAX_STATE: usize = 1024 * 1024;
const MAX_DEPTH: usize = 32;
const MAX_NESTING: usize = 4;
const MAX_ITEMS: usize = 16;
const TOOLS: &[&str] = &[
    "workspace_list", "file_read_text", "file_write_text", "file_delete",
    "file_copy_to_ai", "file_export", "directory_create", "audio_inspect", "nodes_list",
    "graph_validate", "graph_run",
];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WorkflowError {
    pub code: String,
    pub message: String,
    pub step_path: String,
}

impl WorkflowError {
    fn new(code: &str, message: impl Into<String>, path: impl Into<String>) -> Self {
        Self { code: code.into(), message: message.into(), step_path: path.into() }
    }
}
impl fmt::Display for WorkflowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}: {}", self.code, self.step_path, self.message)
    }
}
impl std::error::Error for WorkflowError {}

#[derive(Clone, Debug)]
struct Limits {
    max_steps: usize,
    max_tool_calls: usize,
    max_graph_runs: usize,
    timeout_ms: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self { max_steps: 128, max_tool_calls: 32, max_graph_runs: 8, timeout_ms: 120_000 }
    }
}

#[derive(Clone, Debug)]
enum StepKind {
    Set(Value),
    Compute { op: String, args: Vec<Value> },
    Concat { parts: Vec<Value> },
    Filter { items: Value, condition: Condition },
    Select { items: Value, by: Value, order: String },
    Call { tool: String, args: Value },
    ForEach { items: Value, steps: Vec<Step> },
    If { condition: Condition, then_steps: Vec<Step>, else_steps: Vec<Step> },
}
#[derive(Clone, Debug)]
struct Condition { op: String, left: Value, right: Value }
#[derive(Clone, Debug)]
struct Step { id: String, kind: StepKind, source: String }

#[derive(Clone, Debug)]
pub struct Workflow {
    inputs: Map<String, Value>,
    steps: Vec<Step>,
    outputs: Value,
    limits: Limits,
}

#[derive(Debug, Serialize)]
pub struct WorkflowReport {
    pub schema_version: u8,
    pub state: &'static str,
    pub outputs: Value,
    pub step_results: Value,
    pub trace: Vec<TraceEntry>,
    pub steps_executed: usize,
    pub tool_calls: usize,
    pub graph_runs: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<WorkflowError>,
}
impl WorkflowReport {
    fn empty() -> Self {
        Self {
            schema_version: 1, state: "failed", outputs: json!({}),
            step_results: json!({}), trace: Vec::new(), steps_executed: 0,
            tool_calls: 0, graph_runs: 0, error: None,
        }
    }
}
#[derive(Debug, Serialize)]
pub struct TraceEntry {
    pub step_path: String,
    pub id: String,
    pub step_type: &'static str,
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}

pub trait WorkflowHost: Send {
    fn call<'a>(
        &'a mut self,
        tool: &'a str,
        args: Value,
        cancel: &'a AtomicBool,
        deadline: Instant,
        step_path: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>>;
}

fn object<'a>(v: &'a Value, path: &str) -> Result<&'a Map<String, Value>, WorkflowError> {
    v.as_object().ok_or_else(|| WorkflowError::new("invalid_type", "expected object", path))
}
fn allowed(o: &Map<String, Value>, keys: &[&str], path: &str) -> Result<(), WorkflowError> {
    for k in o.keys() {
        if !keys.contains(&k.as_str()) {
            return Err(WorkflowError::new("unknown_field", format!("unknown field {k}"), format!("{path}/{k}")));
        }
    }
    Ok(())
}
fn required<'a>(o: &'a Map<String, Value>, k: &str, path: &str) -> Result<&'a Value, WorkflowError> {
    o.get(k).ok_or_else(|| WorkflowError::new("missing_field", format!("missing {k}"), format!("{path}/{k}")))
}
fn string<'a>(v: &'a Value, path: &str) -> Result<&'a str, WorkflowError> {
    v.as_str().ok_or_else(|| WorkflowError::new("invalid_type", "expected string", path))
}
fn bytes(v: &Value) -> usize { serde_json::to_vec(v).map_or(usize::MAX, |b| b.len()) }
fn size(v: &Value, max: usize, path: &str) -> Result<(), WorkflowError> {
    if bytes(v) > max { Err(WorkflowError::new("value_limit", format!("JSON value exceeds {max} bytes"), path)) }
    else { Ok(()) }
}
fn depth(v: &Value, n: usize, path: &str) -> Result<(), WorkflowError> {
    if n > MAX_DEPTH { return Err(WorkflowError::new("depth_limit", "JSON depth exceeds 32", path)); }
    match v {
        Value::Array(a) => for (i, x) in a.iter().enumerate() { depth(x, n + 1, &format!("{path}/{i}"))?; },
        Value::Object(o) => for (k, x) in o { depth(x, n + 1, &format!("{path}/{}", escape(k)))?; },
        _ => {}
    }
    Ok(())
}
fn escape(s: &str) -> String { s.replace('~', "~0").replace('/', "~1") }
fn id_valid(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else { return false };
    s.len() <= 64 && first.is_ascii_alphabetic() &&
        chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}
fn pointer_parts(reference: &str, path: &str) -> Result<Vec<String>, WorkflowError> {
    if !reference.starts_with('/') { return Err(WorkflowError::new("invalid_ref", "reference must be a JSON Pointer", path)); }
    reference[1..].split('/').map(|part| {
        let mut out = String::new();
        let mut chars = part.chars();
        while let Some(ch) = chars.next() {
            if ch == '~' {
                match chars.next() {
                    Some('0') => out.push('~'),
                    Some('1') => out.push('/'),
                    _ => return Err(WorkflowError::new("invalid_ref", "invalid JSON Pointer escape", path)),
                }
            } else { out.push(ch); }
        }
        Ok(out)
    }).collect()
}
fn check_ref(reference: &str, inputs: &Map<String, Value>, visible: &HashSet<String>,
             in_loop: bool, path: &str) -> Result<(), WorkflowError> {
    let p = pointer_parts(reference, path)?;
    match p.first().map(String::as_str) {
        Some("inputs") if p.len() >= 2 && inputs.contains_key(&p[1]) => Ok(()),
        Some("steps") if p.len() >= 2 && visible.contains(&p[1]) => Ok(()),
        Some("item") if in_loop => Ok(()),
        Some("index") if in_loop && p.len() == 1 => Ok(()),
        _ => Err(WorkflowError::new("invalid_ref", format!("out-of-scope reference {reference}"), path)),
    }
}
fn check_expr(v: &Value, inputs: &Map<String, Value>, visible: &HashSet<String>,
              in_loop: bool, path: &str, n: usize) -> Result<(), WorkflowError> {
    if n > MAX_DEPTH { return Err(WorkflowError::new("depth_limit", "expression depth exceeds 32", path)); }
    match v {
        Value::Array(a) => for (i, x) in a.iter().enumerate() {
            check_expr(x, inputs, visible, in_loop, &format!("{path}/{i}"), n + 1)?;
        },
        Value::Object(o) if o.len() == 1 && o.contains_key("$ref") => {
            check_ref(string(&o["$ref"], &format!("{path}/$ref"))?, inputs, visible, in_loop, path)?;
        }
        Value::Object(o) if o.len() == 1 && o.contains_key("$literal") => {
            depth(&o["$literal"], n + 1, &format!("{path}/$literal"))?;
        }
        Value::Object(o) => for (k, x) in o {
            check_expr(x, inputs, visible, in_loop, &format!("{path}/{}", escape(k)), n + 1)?;
        },
        _ => {}
    }
    Ok(())
}
fn parse_limits(v: Option<&Value>) -> Result<Limits, WorkflowError> {
    let mut limits = Limits::default();
    let Some(v) = v else { return Ok(limits) };
    let o = object(v, "/limits")?;
    allowed(o, &["max_steps", "max_tool_calls", "max_graph_runs", "timeout_ms"], "/limits")?;
    for (name, target, cap) in [
        ("max_steps", &mut limits.max_steps, 256usize),
        ("max_tool_calls", &mut limits.max_tool_calls, 64),
        ("max_graph_runs", &mut limits.max_graph_runs, 16),
    ] {
        if let Some(v) = o.get(name) {
            let Some(n) = v.as_u64() else { return Err(WorkflowError::new("invalid_limit", "limit must be an unsigned integer", format!("/limits/{name}"))); };
            if n > cap as u64 { return Err(WorkflowError::new("invalid_limit", format!("limit must be 0..={cap}"), format!("/limits/{name}"))); }
            *target = n as usize;
        }
    }
    if let Some(v) = o.get("timeout_ms") {
        let Some(n) = v.as_u64() else { return Err(WorkflowError::new("invalid_limit", "timeout must be an unsigned integer", "/limits/timeout_ms")); };
        if n == 0 || n > 300_000 { return Err(WorkflowError::new("invalid_limit", "timeout must be 1..=300000", "/limits/timeout_ms")); }
        limits.timeout_ms = n;
    }
    Ok(limits)
}
fn parse_steps(v: &Value, inputs: &Map<String, Value>, outer: &HashSet<String>,
               in_loop: bool, nesting: usize, path: &str) -> Result<Vec<Step>, WorkflowError> {
    if nesting > MAX_NESTING { return Err(WorkflowError::new("nesting_limit", "steps nested beyond 4 levels", path)); }
    let a = v.as_array().ok_or_else(|| WorkflowError::new("invalid_type", "steps must be an array", path))?;
    let mut visible = outer.clone();
    let mut result = Vec::with_capacity(a.len());
    for (i, raw) in a.iter().enumerate() {
        let source = format!("{path}/{i}");
        let o = object(raw, &source)?;
        let id = string(required(o, "id", &source)?, &format!("{source}/id"))?;
        if !id_valid(id) { return Err(WorkflowError::new("invalid_id", "id must be 1..=64 ASCII letters, digits, _ or -, starting with a letter", format!("{source}/id"))); }
        if visible.contains(id) { return Err(WorkflowError::new("duplicate_id", format!("id {id} shadows a visible step"), format!("{source}/id"))); }
        let ty = string(required(o, "type", &source)?, &format!("{source}/type"))?;
        let kind = match ty {
            "set" => {
                allowed(o, &["id", "type", "value"], &source)?;
                let v = required(o, "value", &source)?;
                check_expr(v, inputs, &visible, in_loop, &format!("{source}/value"), 0)?;
                StepKind::Set(v.clone())
            }
            "compute" => {
                allowed(o, &["id", "type", "op", "args"], &source)?;
                let op_path = format!("{source}/op");
                let op = string(required(o, "op", &source)?, &op_path)?;
                if !["add", "sub", "mul", "div", "abs"].contains(&op) {
                    return Err(WorkflowError::new("invalid_operator", format!("unsupported operator {op}"), op_path));
                }
                let args_path = format!("{source}/args");
                let args_value = required(o, "args", &source)?;
                let args = args_value.as_array().ok_or_else(|| WorkflowError::new("invalid_type", "compute args must be an array", &args_path))?;
                let arity = if op == "abs" { 1 } else { 2 };
                if args.len() != arity {
                    return Err(WorkflowError::new("invalid_arity", format!("{op} requires exactly {arity} arguments"), args_path));
                }
                check_expr(args_value, inputs, &visible, in_loop, &args_path, 0)?;
                StepKind::Compute { op: op.into(), args: args.clone() }
            }
            "concat" => {
                allowed(o, &["id", "type", "parts"], &source)?;
                let parts_path = format!("{source}/parts");
                let parts_value = required(o, "parts", &source)?;
                let parts = parts_value.as_array().ok_or_else(|| WorkflowError::new("invalid_type", "concat parts must be an array", &parts_path))?;
                if parts.len() > 64 {
                    return Err(WorkflowError::new("item_limit", "concat permits at most 64 parts", parts_path));
                }
                check_expr(parts_value, inputs, &visible, in_loop, &parts_path, 0)?;
                StepKind::Concat { parts: parts.clone() }
            }
            "call" => {
                allowed(o, &["id", "type", "tool", "args"], &source)?;
                let tool = string(required(o, "tool", &source)?, &format!("{source}/tool"))?;
                if !TOOLS.contains(&tool) { return Err(WorkflowError::new("unknown_tool", format!("tool {tool} is not allowed"), format!("{source}/tool"))); }
                let args = required(o, "args", &source)?;
                object(args, &format!("{source}/args"))?;
                check_expr(args, inputs, &visible, in_loop, &format!("{source}/args"), 0)?;
                StepKind::Call { tool: tool.into(), args: args.clone() }
            }
            "filter" => {
                allowed(o, &["id", "type", "items", "condition"], &source)?;
                let items = required(o, "items", &source)?;
                check_expr(items, inputs, &visible, in_loop, &format!("{source}/items"), 0)?;
                let condition = parse_condition(required(o, "condition", &source)?, inputs, &visible, true, &format!("{source}/condition"))?;
                StepKind::Filter { items: items.clone(), condition }
            }
            "select" => {
                allowed(o, &["id", "type", "items", "by", "order"], &source)?;
                let items = required(o, "items", &source)?;
                check_expr(items, inputs, &visible, in_loop, &format!("{source}/items"), 0)?;
                let by = required(o, "by", &source)?;
                check_expr(by, inputs, &visible, true, &format!("{source}/by"), 0)?;
                let order_path = format!("{source}/order");
                let order = string(required(o, "order", &source)?, &order_path)?;
                if !["min", "max"].contains(&order) {
                    return Err(WorkflowError::new("invalid_order", format!("unsupported order {order}"), order_path));
                }
                StepKind::Select { items: items.clone(), by: by.clone(), order: order.into() }
            }
            "for_each" => {
                allowed(o, &["id", "type", "items", "steps"], &source)?;
                let items = required(o, "items", &source)?;
                check_expr(items, inputs, &visible, in_loop, &format!("{source}/items"), 0)?;
                let steps = parse_steps(required(o, "steps", &source)?, inputs, &visible, true, nesting + 1, &format!("{source}/steps"))?;
                StepKind::ForEach { items: items.clone(), steps }
            }
            "if" => {
                allowed(o, &["id", "type", "condition", "then", "else"], &source)?;
                let cond_path = format!("{source}/condition");
                let cond = object(required(o, "condition", &source)?, &cond_path)?;
                allowed(cond, &["op", "left", "right"], &cond_path)?;
                let op = string(required(cond, "op", &cond_path)?, &format!("{cond_path}/op"))?;
                if !["eq", "ne", "lt", "lte", "gt", "gte"].contains(&op) {
                    return Err(WorkflowError::new("invalid_operator", format!("unsupported operator {op}"), format!("{cond_path}/op")));
                }
                let left = required(cond, "left", &cond_path)?;
                let right = required(cond, "right", &cond_path)?;
                check_expr(left, inputs, &visible, in_loop, &format!("{cond_path}/left"), 0)?;
                check_expr(right, inputs, &visible, in_loop, &format!("{cond_path}/right"), 0)?;
                let then_steps = parse_steps(required(o, "then", &source)?, inputs, &visible, in_loop, nesting + 1, &format!("{source}/then"))?;
                let else_steps = match o.get("else") {
                    Some(v) => parse_steps(v, inputs, &visible, in_loop, nesting + 1, &format!("{source}/else"))?,
                    None => Vec::new(),
                };
                StepKind::If { condition: Condition { op: op.into(), left: left.clone(), right: right.clone() }, then_steps, else_steps }
            }
            _ => return Err(WorkflowError::new("unknown_step", format!("unsupported step type {ty}"), format!("{source}/type"))),
        };
        result.push(Step { id: id.into(), kind, source });
        visible.insert(id.into());
    }
    Ok(result)
}

fn parse_condition(v: &Value, inputs: &Map<String, Value>, visible: &HashSet<String>,
                   in_loop: bool, path: &str) -> Result<Condition, WorkflowError> {
    let o = object(v, path)?;
    allowed(o, &["op", "left", "right"], path)?;
    let op = string(required(o, "op", path)?, &format!("{path}/op"))?;
    if !["eq", "ne", "lt", "lte", "gt", "gte"].contains(&op) {
        return Err(WorkflowError::new("invalid_operator", format!("unsupported operator {op}"), format!("{path}/op")));
    }
    let left = required(o, "left", path)?;
    let right = required(o, "right", path)?;
    check_expr(left, inputs, visible, in_loop, &format!("{path}/left"), 0)?;
    check_expr(right, inputs, visible, in_loop, &format!("{path}/right"), 0)?;
    Ok(Condition { op: op.into(), left: left.clone(), right: right.clone() })
}

fn compare_condition(op: &str, left: &Value, right: &Value,
                     left_path: &str, right_path: &str) -> Result<bool, WorkflowError> {
    Ok(match op {
        "eq" => left == right,
        "ne" => left != right,
        op => {
            let a = left.as_f64().filter(|v| v.is_finite()).ok_or_else(|| WorkflowError::new("invalid_type", "ordered comparison requires finite numbers", left_path))?;
            let b = right.as_f64().filter(|v| v.is_finite()).ok_or_else(|| WorkflowError::new("invalid_type", "ordered comparison requires finite numbers", right_path))?;
            match op { "lt" => a < b, "lte" => a <= b, "gt" => a > b, "gte" => a >= b, _ => unreachable!() }
        }
    })
}

pub fn validate(document: &Value) -> Result<Workflow, WorkflowError> {
    size(document, MAX_DOCUMENT, "")?;
    depth(document, 0, "")?;
    let o = object(document, "")?;
    allowed(o, &["schema_version", "inputs", "steps", "outputs", "limits"], "")?;
    if required(o, "schema_version", "")?.as_u64() != Some(1) {
        return Err(WorkflowError::new("schema_version", "schema_version must be integer 1", "/schema_version"));
    }
    let inputs = object(required(o, "inputs", "")?, "/inputs")?.clone();
    let limits = parse_limits(o.get("limits"))?;
    let steps = parse_steps(required(o, "steps", "")?, &inputs, &HashSet::new(), false, 0, "/steps")?;
    let visible: HashSet<String> = steps.iter().map(|s| s.id.clone()).collect();
    let outputs = required(o, "outputs", "")?;
    object(outputs, "/outputs")?;
    check_expr(outputs, &inputs, &visible, false, "/outputs", 0)?;
    Ok(Workflow { inputs, steps, outputs: outputs.clone(), limits })
}

pub fn validation_report(workflow: &Workflow) -> Value {
    json!({
        "schema_version": 1, "valid": true,
        "steps": workflow.steps.len(),
        "limits": {
            "max_steps": workflow.limits.max_steps,
            "max_tool_calls": workflow.limits.max_tool_calls,
            "max_graph_runs": workflow.limits.max_graph_runs,
            "timeout_ms": workflow.limits.timeout_ms
        }
    })
}

struct Context {
    inputs: Value,
    frames: Vec<Map<String, Value>>,
    item: Option<Value>,
    index: Option<usize>,
}
impl Context {
    fn visible_step(&self, id: &str) -> Option<&Value> {
        self.frames.iter().rev().find_map(|f| f.get(id))
    }
    fn resolve(&self, reference: &str, path: &str) -> Result<Value, WorkflowError> {
        let p = pointer_parts(reference, path)?;
        let root = match p.first().map(String::as_str) {
            Some("inputs") => &self.inputs,
            Some("steps") if p.len() >= 2 => self.visible_step(&p[1]).ok_or_else(|| WorkflowError::new("invalid_ref", "step is unavailable", path))?,
            Some("item") => self.item.as_ref().ok_or_else(|| WorkflowError::new("invalid_ref", "item is unavailable", path))?,
            Some("index") => return if p.len() == 1 {
                self.index.map(|i| json!(i)).ok_or_else(|| WorkflowError::new("invalid_ref", "index is unavailable", path))
            } else { Err(WorkflowError::new("invalid_ref", "index has no children", path)) },
            _ => return Err(WorkflowError::new("invalid_ref", "unsupported reference", path)),
        };
        let parts = if p[0] == "steps" { &p[2..] } else { &p[1..] };
        let mut current = root;
        for part in parts {
            current = match current {
                Value::Object(o) => o.get(part),
                Value::Array(a) => part.parse::<usize>().ok()
                    .filter(|i| i.to_string() == *part).and_then(|i| a.get(i)),
                _ => None,
            }.ok_or_else(|| WorkflowError::new("missing_ref", format!("reference {reference} does not resolve"), path))?;
        }
        Ok(current.clone())
    }
    fn eval(&self, expr: &Value, path: &str) -> Result<Value, WorkflowError> {
        let result = match expr {
            Value::Object(o) if o.len() == 1 && o.contains_key("$ref") =>
                self.resolve(o["$ref"].as_str().unwrap_or_default(), path)?,
            Value::Object(o) if o.len() == 1 && o.contains_key("$literal") => o["$literal"].clone(),
            Value::Object(o) => {
                let mut m = Map::new();
                let mut used = 2usize;
                for (k, v) in o {
                    let child = self.eval(v, path)?;
                    used = used.saturating_add(bytes(&Value::String(k.clone())))
                        .saturating_add(bytes(&child))
                        .saturating_add(1 + usize::from(!m.is_empty()));
                    if used > MAX_VALUE {
                        return Err(WorkflowError::new("value_limit", "evaluated object exceeds 256 KiB", path));
                    }
                    m.insert(k.clone(), child);
                }
                Value::Object(m)
            }
            Value::Array(a) => {
                let mut values = Vec::with_capacity(a.len());
                let mut used = 2usize;
                for v in a {
                    let child = self.eval(v, path)?;
                    used = used.saturating_add(bytes(&child))
                        .saturating_add(usize::from(!values.is_empty()));
                    if used > MAX_VALUE {
                        return Err(WorkflowError::new("value_limit", "evaluated array exceeds 256 KiB", path));
                    }
                    values.push(child);
                }
                Value::Array(values)
            },
            _ => expr.clone(),
        };
        size(&result, MAX_VALUE, path)?;
        Ok(result)
    }
}

struct Runner<'a> {
    host: &'a mut dyn WorkflowHost,
    cancel: &'a AtomicBool,
    deadline: Instant,
    limits: &'a Limits,
    report: WorkflowReport,
}

fn compute_number(value: &Value, path: &str) -> Result<f64, WorkflowError> {
    const MAX_EXACT_INTEGER: u64 = (1u64 << 53) - 1;
    let number = value.as_number().ok_or_else(|| WorkflowError::new("invalid_type", "compute requires finite numbers", path))?;
    if number.as_i64().is_some_and(|n| n < -(MAX_EXACT_INTEGER as i64) || n > MAX_EXACT_INTEGER as i64)
        || number.as_u64().is_some_and(|n| n > MAX_EXACT_INTEGER) {
        return Err(WorkflowError::new("numeric_precision", "integer exceeds the exact f64 input range", path));
    }
    number.as_f64().filter(|n| n.is_finite())
        .ok_or_else(|| WorkflowError::new("invalid_type", "compute requires finite numbers", path))
}

// Content bytes only; the caller separately accounts for the two JSON quotes.
fn json_string_content_bytes(value: &str) -> usize {
    value.bytes().map(|byte| match byte {
        b'"' | b'\\' | b'\x08' | b'\t' | b'\n' | b'\x0c' | b'\r' => 2,
        0..=31 => 6,
        _ => 1,
    }).sum()
}

impl Runner<'_> {
    fn check(&self, path: &str) -> Result<(), WorkflowError> {
        if self.cancel.load(Ordering::Acquire) {
            Err(WorkflowError::new("cancelled", "workflow was cancelled", path))
        } else if Instant::now() >= self.deadline {
            Err(WorkflowError::new("timeout", "workflow deadline exceeded", path))
        } else { Ok(()) }
    }
    fn state_size(&self, ctx: &Context, path: &str) -> Result<(), WorkflowError> {
        let mut total = serde_json::to_vec(&self.report).map_or(usize::MAX, |b| b.len());
        total = total.saturating_add(bytes(&ctx.inputs));
        for frame in &ctx.frames { total = total.saturating_add(bytes(&Value::Object(frame.clone()))); }
        if let Some(item) = &ctx.item { total = total.saturating_add(bytes(item)); }
        if total > MAX_STATE {
            Err(WorkflowError::new("state_limit", "workflow state/report exceeds 1 MiB", path))
        } else { Ok(()) }
    }
    fn run_steps<'b>(&'b mut self, steps: &'b [Step], ctx: &'b mut Context,
                      path: String) -> Pin<Box<dyn Future<Output = Result<Map<String, Value>, WorkflowError>> + Send + 'b>> {
        Box::pin(async move {
            ctx.frames.push(Map::new());
            for step in steps {
                let step_path = format!("{path}/{}:{}", step.source.rsplit('/').next().unwrap_or("0"), step.id);
                let result = self.run_step(step, ctx, &step_path).await;
                match result {
                    Ok(value) => {
                        ctx.frames.last_mut().expect("frame").insert(step.id.clone(), value.clone());
                        if ctx.frames.len() == 1 {
                            self.report.step_results = Value::Object(ctx.frames[0].clone());
                        }
                        self.report.trace.push(TraceEntry {
                            step_path: step_path.clone(), id: step.id.clone(),
                            step_type: step.kind.name(), state: "succeeded",
                            tool: step.kind.tool().map(str::to_owned),
                            result: Some(value),
                        });
                        if let Err(err) = self.state_size(ctx, &step_path) {
                            // The last addition did not fit. Keep the earlier, bounded
                            // partial report and report this step as a limit failure.
                            self.report.trace.pop();
                            ctx.frames.last_mut().expect("frame").remove(&step.id);
                            if ctx.frames.len() == 1 {
                                self.report.step_results = Value::Object(ctx.frames[0].clone());
                            }
                            self.push_failure(step, &step_path);
                            ctx.frames.pop();
                            return Err(err);
                        }
                    }
                    Err(err) => {
                        self.push_failure(step, &step_path);
                        ctx.frames.pop();
                        return Err(err);
                    }
                }
            }
            Ok(ctx.frames.pop().expect("frame"))
        })
    }
    fn push_failure(&mut self, step: &Step, path: &str) {
        self.report.trace.push(TraceEntry {
            step_path: path.into(), id: step.id.clone(), step_type: step.kind.name(),
            state: "failed", tool: step.kind.tool().map(str::to_owned), result: None,
        });
    }
    async fn run_step(&mut self, step: &Step, ctx: &mut Context, path: &str) -> Result<Value, WorkflowError> {
        self.check(path)?;
        if self.report.steps_executed >= self.limits.max_steps {
            return Err(WorkflowError::new("step_limit", "step budget exhausted", path));
        }
        self.report.steps_executed += 1;
        let value = match &step.kind {
            StepKind::Set(expr) => ctx.eval(expr, path)?,
            StepKind::Compute { op, args } => {
                let mut numbers = Vec::with_capacity(args.len());
                for (i, expr) in args.iter().enumerate() {
                    let arg_path = format!("{path}/args/{i}");
                    self.check(&arg_path)?;
                    numbers.push(compute_number(&ctx.eval(expr, &arg_path)?, &arg_path)?);
                }
                let result = match op.as_str() {
                    "add" => numbers[0] + numbers[1],
                    "sub" => numbers[0] - numbers[1],
                    "mul" => numbers[0] * numbers[1],
                    "div" => {
                        if numbers[1] == 0.0 {
                            return Err(WorkflowError::new("division_by_zero", "division by zero", format!("{path}/args/1")));
                        }
                        numbers[0] / numbers[1]
                    }
                    "abs" => numbers[0].abs(),
                    _ => unreachable!(),
                };
                let number = serde_json::Number::from_f64(result).ok_or_else(|| WorkflowError::new("numeric_overflow", "compute result is not finite", format!("{path}/op")))?;
                Value::Number(number)
            }
            StepKind::Concat { parts } => {
                let mut result = String::new();
                let mut used = 2usize;
                for (i, expr) in parts.iter().enumerate() {
                    let part_path = format!("{path}/parts/{i}");
                    self.check(&part_path)?;
                    let value = ctx.eval(expr, &part_path)?;
                    let text = match value {
                        Value::String(text) => text,
                        Value::Number(number) => number.to_string(),
                        _ => return Err(WorkflowError::new("invalid_type", "concat parts must be strings or numbers", part_path)),
                    };
                    used = used.saturating_add(json_string_content_bytes(&text));
                    if used > MAX_VALUE {
                        return Err(WorkflowError::new("value_limit", "concatenated string exceeds 256 KiB", part_path));
                    }
                    result.push_str(&text);
                }
                Value::String(result)
            }
            StepKind::Call { tool, args } => {
                if self.report.tool_calls >= self.limits.max_tool_calls {
                    return Err(WorkflowError::new("tool_limit", "tool call budget exhausted", path));
                }
                if tool == "graph_run" && self.report.graph_runs >= self.limits.max_graph_runs {
                    return Err(WorkflowError::new("graph_limit", "graph run budget exhausted", path));
                }
                let args = ctx.eval(args, path)?;
                object(&args, path)?;
                self.check(path)?;
                self.report.tool_calls += 1;
                if tool == "graph_run" { self.report.graph_runs += 1; }
                let result = self.host.call(tool, args, self.cancel, self.deadline, path).await;
                self.check(path)?;
                let value = result.map_err(|message| WorkflowError::new("tool_failed", message, path))?;
                object(&value, path)?;
                value
            }
            StepKind::Filter { items, condition } => {
                let items_path = format!("{path}/items");
                let items = ctx.eval(items, &items_path)?;
                let array = items.as_array().ok_or_else(|| WorkflowError::new("invalid_type", "filter items must be an array", &items_path))?;
                if array.len() > MAX_ITEMS { return Err(WorkflowError::new("item_limit", "filter permits at most 16 items", &items_path)); }
                let mut collected = Vec::with_capacity(array.len());
                let mut used = 2usize;
                for (i, item) in array.iter().enumerate() {
                    let item_path = format!("{items_path}/{i}");
                    self.check(&item_path)?;
                    let old_item = ctx.item.replace(item.clone());
                    let old_index = ctx.index.replace(i);
                    let chosen = (|| {
                        let left_path = format!("{item_path}/condition/left");
                        let right_path = format!("{item_path}/condition/right");
                        let left = ctx.eval(&condition.left, &left_path)?;
                        let right = ctx.eval(&condition.right, &right_path)?;
                        compare_condition(&condition.op, &left, &right, &left_path, &right_path)
                    })();
                    ctx.item = old_item;
                    ctx.index = old_index;
                    if chosen? {
                        used = used.saturating_add(bytes(item)).saturating_add(usize::from(!collected.is_empty()));
                        if used > MAX_VALUE {
                            return Err(WorkflowError::new("value_limit", "filtered array exceeds 256 KiB", &item_path));
                        }
                        collected.push(item.clone());
                    }
                }
                Value::Array(collected)
            }
            StepKind::Select { items, by, order } => {
                let items_path = format!("{path}/items");
                let items = ctx.eval(items, &items_path)?;
                let array = items.as_array().ok_or_else(|| WorkflowError::new("invalid_type", "select items must be an array", &items_path))?;
                if array.len() > MAX_ITEMS { return Err(WorkflowError::new("item_limit", "select permits at most 16 items", &items_path)); }
                let mut best: Option<(usize, f64, Value)> = None;
                for (i, item) in array.iter().enumerate() {
                    let item_path = format!("{items_path}/{i}");
                    self.check(&item_path)?;
                    let old_item = ctx.item.replace(item.clone());
                    let old_index = ctx.index.replace(i);
                    let score_path = format!("{item_path}/by");
                    let score = ctx.eval(by, &score_path)
                        .and_then(|value| compute_number(&value, &score_path).map(|number| (number, value)));
                    ctx.item = old_item;
                    ctx.index = old_index;
                    let (number, value) = score?;
                    let better = best.as_ref().map_or(true, |(_, previous, _)| {
                        if order == "min" { number < *previous } else { number > *previous }
                    });
                    if better { best = Some((i, number, value)); }
                }
                match best {
                    Some((index, _, score)) => json!({"found": true, "item": array[index].clone(), "index": index, "score": score}),
                    None => json!({"found": false, "item": null, "index": null, "score": null}),
                }
            }
            StepKind::ForEach { items, steps } => {
                let items = ctx.eval(items, path)?;
                let array = items.as_array().ok_or_else(|| WorkflowError::new("invalid_type", "for_each items must be an array", path))?;
                if array.len() > MAX_ITEMS { return Err(WorkflowError::new("item_limit", "for_each permits at most 16 items", path)); }
                let mut collected = Vec::with_capacity(array.len());
                for (i, item) in array.iter().enumerate() {
                    self.check(path)?;
                    let old_item = ctx.item.replace(item.clone());
                    let old_index = ctx.index.replace(i);
                    let result = self.run_steps(steps, ctx, format!("{path}/iterations/{i}")).await;
                    ctx.item = old_item;
                    ctx.index = old_index;
                    collected.push(Value::Object(result?));
                    size(&Value::Array(collected.clone()), MAX_VALUE, path)?;
                }
                Value::Array(collected)
            }
            StepKind::If { condition, then_steps, else_steps } => {
                let left = ctx.eval(&condition.left, path)?;
                let right = ctx.eval(&condition.right, path)?;
                let chosen = compare_condition(&condition.op, &left, &right, path, path)?;
                let branch = if chosen { "then" } else { "else" };
                let branch_steps = if chosen { then_steps } else { else_steps };
                let results = self.run_steps(branch_steps, ctx, format!("{path}/{branch}")).await?;
                json!({"branch": branch, "steps": results})
            }
        };
        size(&value, MAX_VALUE, path)?;
        self.check(path)?;
        Ok(value)
    }
}
impl StepKind {
    fn name(&self) -> &'static str {
        match self { Self::Set(_) => "set", Self::Compute { .. } => "compute", Self::Concat { .. } => "concat", Self::Filter { .. } => "filter", Self::Select { .. } => "select", Self::Call { .. } => "call", Self::ForEach { .. } => "for_each", Self::If { .. } => "if" }
    }
    fn tool(&self) -> Option<&str> {
        match self { Self::Call { tool, .. } => Some(tool), _ => None }
    }
}

pub async fn run(workflow: &Workflow, inputs: Option<&Value>, host: &mut dyn WorkflowHost,
                 cancel: &AtomicBool) -> WorkflowReport {
    let mut report = WorkflowReport::empty();
    let mut merged = workflow.inputs.clone();
    if let Some(overrides) = inputs {
        let Some(o) = overrides.as_object() else {
            report.error = Some(WorkflowError::new("invalid_type", "input overrides must be an object", "/inputs"));
            return report;
        };
        for (k, v) in o {
            if !merged.contains_key(k) {
                report.error = Some(WorkflowError::new("unknown_input", format!("undeclared input {k}"), format!("/inputs/{}", escape(k))));
                return report;
            }
            merged.insert(k.clone(), v.clone());
        }
    }
    let input_value = Value::Object(merged);
    if let Err(err) = depth(&input_value, 0, "/inputs").and_then(|_| size(&input_value, MAX_VALUE, "/inputs")) {
        report.error = Some(err);
        report.state = "limited";
        return report;
    }
    let mut ctx = Context { inputs: input_value, frames: Vec::new(), item: None, index: None };
    let deadline = Instant::now() + Duration::from_millis(workflow.limits.timeout_ms);
    let mut runner = Runner { host, cancel, deadline, limits: &workflow.limits, report };
    let result = runner.check("/steps")
        .and_then(|_| runner.state_size(&ctx, "/inputs"));
    let result = match result {
        Ok(()) => runner.run_steps(&workflow.steps, &mut ctx, "/steps".into()).await,
        Err(err) => Err(err),
    }
        .and_then(|steps| {
            ctx.frames.push(steps);
            let outputs = ctx.eval(&workflow.outputs, "/outputs")?;
            object(&outputs, "/outputs")?;
            runner.check("/outputs")?;
            runner.report.outputs = outputs.clone();
            runner.state_size(&ctx, "/outputs")?;
            Ok(outputs)
        });
    match result {
        Ok(outputs) => { runner.report.outputs = outputs; runner.report.state = "succeeded"; }
        Err(err) => {
            runner.report.outputs = json!({});
            runner.report.state = match err.code.as_str() {
                "cancelled" => "cancelled",
                "timeout" | "step_limit" | "tool_limit" | "graph_limit" | "item_limit" | "value_limit" | "state_limit" | "depth_limit" => "limited",
                _ => "failed",
            };
            runner.report.error = Some(err);
        }
    }
    if serde_json::to_vec(&runner.report).map_or(true, |b| b.len() > MAX_STATE) {
        runner.report.state = "limited";
        runner.report.outputs = json!({});
        runner.report.error = Some(WorkflowError::new("state_limit", "workflow report exceeds 1 MiB", ""));
        while serde_json::to_vec(&runner.report).map_or(true, |b| b.len() > MAX_STATE) {
            if runner.report.trace.pop().is_none() { break; }
        }
    }
    runner.report
}

#[cfg(test)]
#[path = "workflow_tests.rs"]
mod workflow_tests;

#[cfg(test)]
#[path = "workflow_value_tests.rs"]
mod workflow_value_tests;

#[cfg(test)]
#[path = "workflow_selection_tests.rs"]
mod workflow_selection_tests;
