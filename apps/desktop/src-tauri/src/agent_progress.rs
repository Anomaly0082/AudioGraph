//! Bounded loop protection and factual stop receipts; no extra model request.
use super::AgentEvent;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct FailureGuard { failures: HashMap<String, usize> }

fn key(name: &str, args: &Value) -> String {
    format!("{:x}", Sha256::digest(format!("{name}\0{args}").as_bytes()))
}

fn mutation(name: &str) -> bool {
    matches!(name, "file_write_text" | "file_delete" | "file_copy_to_ai" |
        "file_export" | "directory_create" | "audio_convert" | "graph_run" | "workflow_run")
}

impl FailureGuard {
    pub fn blocked(&self, name: &str, args: &Value) -> bool {
        self.failures.get(&key(name,args)).copied().unwrap_or(0) >= 2
    }
    pub fn observe(&mut self, name: &str, args: Option<&Value>, success: bool) {
        if success {
            // A successful mutation may repair the environment for an identical call.
            // Read-only inspection does not itself fix a failed request.
            if mutation(name) {
                self.failures.clear();
            }
        } else if let Some(args) = args {
            *self.failures.entry(key(name,args)).or_default() += 1;
        }
    }
    pub fn observe_receipt(&mut self, name: &str, args: Option<&Value>, success: bool, envelope: &Value) {
        // Failed workflows may have completed mutations before a later failure.
        if name == "workflow_run" && envelope.pointer("/data/trace").and_then(Value::as_array)
            .is_some_and(|trace| trace.iter().any(|step| step["step_type"] == "call"
                && step["state"] == "succeeded" && step["tool"].as_str().is_some_and(mutation))) {
            self.failures.clear();
        }
        self.observe(name,args,success);
    }
}

pub(super) fn budget_notice(request: &mut Value, model_calls: usize, tool_calls: usize) {
    let models = super::MAX_MODEL_CALLS.saturating_sub(model_calls);
    let tools = super::MAX_TOOL_CALLS.saturating_sub(tool_calls);
    let notice = format!("\nTurn budget including this request: {models} model requests and {tools} tool calls remain. Use existing receipts; do not re-read complete results or explore unrelated history/files. {}", if models <= 3 || tools <= 4 {
        "Finish the requested output/export if feasible, otherwise clearly summarize confirmed work, remaining work and the blocker. Do not start optional probes."
    } else { "Group independent necessary inspections into one response when possible." });
    if let Some(system) = request.pointer_mut("/messages/0/content") {
        if let Some(text) = system.as_str() { *system = Value::String(format!("{text}{notice}")); }
    }
}

fn label(text: &str) -> String {
    // Tool-supplied filenames/errors remain text, not Markdown/HTML instructions.
    super::bounded_text(&text.chars().map(|c| if c.is_control() {' '} else {c})
        .collect::<String>().replace('&', "&amp;").replace('<', "&lt;")
        .replace('>', "&gt;").replace('`', "\\`").replace('*', "\\*")
        .replace('_', "\\_").replace('[', "\\[").replace(']', "\\]"), 400)
}

#[derive(Default)]
struct Confirmed { saved: Vec<String>, exported: Vec<String>, files: Vec<String>, graph_ok: usize,
    graph_failed: usize, workflow_ok: usize, workflow_failed: usize, converted: usize }
fn add(items: &mut Vec<String>, path: Option<&str>) {
    if let Some(path) = path { let path=label(path); if !items.contains(&path) { items.push(path); } }
}
impl Confirmed {
    fn tool(&mut self, name: &str, data: &Value, success: bool) {
        match name {
            "graph_run" => {
                if success && data["state"] == "succeeded" {
                    self.graph_ok += 1;
                    if let Some(outputs) = data.pointer("/result/outputs").and_then(Value::as_object) {
                        for output in outputs.values() {
                            if output["type"] == "FilePath" { add(&mut self.files, output["value"].as_str()); }
                        }
                    }
                } else { self.graph_failed += 1; }
            }
            "workflow_run" => {
                if success && data["state"] == "succeeded" { self.workflow_ok += 1; }
                else { self.workflow_failed += 1; }
                // Only runtime trace receipts, never declared outputs or configuration.
                if let Some(trace) = data["trace"].as_array() {
                    for step in trace.iter().take(256) {
                        if step["step_type"] == "call" && step["state"] == "succeeded" {
                            if let Some(tool) = step["tool"].as_str().filter(|s| *s != "workflow_run") {
                                self.tool(tool,&step["result"],true);
                            }
                        }
                    }
                }
            }
            "file_write_text" if success => add(&mut self.saved,data["path"].as_str()),
            "file_export" if success => add(&mut self.exported,data["path"].as_str()),
            "audio_convert" if success => { self.converted += 1; add(&mut self.files,data["path"].as_str()); }
            _ => {}
        }
    }
}

pub(super) fn stop_summary(reason: &str, events: &[AgentEvent], models: usize, tools: usize,
    unexecuted: usize) -> String {
    let mut confirmed=Confirmed::default();
    let mut last_error=None;
    for event in events {
        if let (Some(name),Some(envelope),Some(success)) = (&event.tool,&event.result,event.success) {
            confirmed.tool(name,&envelope["data"],success);
            if !success { last_error=envelope.get("error").map(|error| label(error.as_str().unwrap_or(&error.to_string()))); }
        }
    }
    let mut lines=vec![format!("本轮已停止：{reason}。已请求模型 {models} 次、调用工具 {tools} 次。"),
        "\n**已确认的执行情况**".into(),
        format!("- Graph：成功 {} 次，失败或未确认成功 {} 次；Workflow：成功 {} 次，失败或未确认成功 {} 次。",confirmed.graph_ok,confirmed.graph_failed,confirmed.workflow_ok,confirmed.workflow_failed)];
    if confirmed.converted>0 { lines.push(format!("- 格式转换：成功 {} 次。",confirmed.converted)); }
    for (title,paths) in [("已写入文本／配置（不代表已执行）",&confirmed.saved),
        ("工具报告的输出路径",&confirmed.files),("已导出到用户工作区",&confirmed.exported)] {
        if !paths.is_empty() {
            lines.push(format!("- {title}：{}{}",paths.iter().take(6).cloned().collect::<Vec<_>>().join("；"),
                if paths.len()>6 {format!("；另有 {} 项见调用详情",paths.len()-6)} else {String::new()}));
        }
    }
    lines.push("\n**尚未确认的部分**".into());
    lines.push("- 上述成功记录可能包含中间步骤或测试，不能据此认定用户要求的最终目标已完成；文件也可能被后续步骤修改或删除。".into());
    if confirmed.exported.is_empty() { lines.push("- 本轮记录中没有成功导出到用户工作区的凭据。".into()); }
    if unexecuted>0 { lines.push(format!("- 还有 {unexecuted} 个已提出的工具调用未执行。")); }
    if let Some(error)=last_error { lines.push(format!("- 最近一次工具失败：{error}")); }
    lines.push("\n可查看调用详情和关联运行记录确认现有产物；继续时应从未完成的步骤处理，避免重复执行已成功的步骤。".into());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn event(name: &str, data: Value, success: bool) -> AgentEvent {
        AgentEvent::tool(name.into(),None,json!({"ok":success,"data":data}),success)
    }
    #[test]
    fn repeated_failure_blocks_only_unchanged_calls_and_repairs_release_it() {
        let mut guard=FailureGuard::default();
        let args=json!({"path":"flow.json"});
        guard.observe("workflow_run",Some(&args),false);
        assert!(!guard.blocked("workflow_run",&args));
        guard.observe("runs_read",Some(&json!({"id":"known"})),true);
        guard.observe("workflow_run",Some(&args),false);
        assert!(guard.blocked("workflow_run",&args));
        assert!(!guard.blocked("workflow_validate",&args));
        assert!(!guard.blocked("workflow_run",&json!({"path":"fixed.json"})));
        guard.observe("file_write_text",Some(&json!({"path":"flow.json"})),false);
        assert!(guard.blocked("workflow_run",&args));
        guard.observe("file_write_text",Some(&json!({"path":"flow.json"})),true);
        assert!(!guard.blocked("workflow_run",&args));
    }
    #[test]
    fn partially_failed_workflow_can_repair_a_previously_failing_tool() {
        let mut guard=FailureGuard::default();
        let args=json!({"path":"repaired.txt"});
        for _ in 0..2 { guard.observe("file_read_text",Some(&args),false); }
        assert!(guard.blocked("file_read_text",&args));
        let partial=json!({"ok":false,"data":{"trace":[
            {"step_type":"call","state":"succeeded","tool":"file_write_text","result":{"path":"repaired.txt"}},
            {"step_type":"call","state":"failed","tool":"graph_run"}
        ]}});
        guard.observe_receipt("workflow_run",Some(&json!({"path":"flow.json"})),false,&partial);
        assert!(!guard.blocked("file_read_text",&args));
    }
    #[test]
    fn stop_receipt_does_not_promote_saved_configuration_or_failed_output_to_delivery() {
        let events=vec![event("file_write_text",json!({"path":"unfinished.workflow.json"}),true),
            event("graph_run",json!({"state":"failed","result":{"outputs":{"file":{"type":"FilePath","value":"must-not-claim.wav"}}}}),false),
            event("file_export",json!({"path":"not-exported.wav"}),false),
            AgentEvent::message("assistant","I finished everything (unverified)".into())];
        let text=stop_summary("达到上限",&events,16,21,1);
        assert!(text.contains("unfinished.workflow.json"));
        assert!(text.contains("不代表已执行"));
        assert!(text.contains("没有成功导出"));
        assert!(text.contains("1 个已提出的工具调用未执行"));
        assert!(!text.contains("must-not-claim.wav"));
        assert!(!text.contains("not-exported.wav"));
        assert!(!text.contains("I finished everything"));
    }
    #[test]
    fn partial_workflow_reports_only_confirmed_trace_exports() {
        let events=vec![event("workflow_run",json!({"state":"failed","outputs":{"fake":"not-real.wav"},"trace":[
            {"step_type":"call","state":"succeeded","tool":"file_export","result":{"path":"batch/real.wav"}},
            {"step_type":"call","state":"failed","tool":"file_export","result":{"path":"failed.wav"}},
            {"step_type":"set","state":"succeeded","tool":"file_export","result":{"path":"pretend.wav"}}
        ]}),false)];
        let text=stop_summary("达到上限",&events,16,20,0);
        assert!(text.contains("batch/real.wav"));
        assert!(text.contains("Workflow：成功 0 次"));
        for absent in ["not-real.wav","failed.wav","pretend.wav","没有成功导出"] { assert!(!text.contains(absent),"{text}"); }
    }
    #[test]
    fn final_request_includes_remaining_budget_without_changing_protocol() {
        let mut request=json!({"messages":[{"role":"system","content":"policy"},{"role":"tool","tool_call_id":"one","content":"result"}]});
        budget_notice(&mut request,15,39);
        assert!(request["messages"][0]["content"].as_str().unwrap().contains("1 model requests and 1 tool calls"));
        assert_eq!(request["messages"][1]["tool_call_id"],"one");
    }
}
