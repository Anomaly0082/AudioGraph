use crate::workflow::{self, WorkflowHost};
use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

#[derive(Default)]
struct ReviewHost {
    calls: usize,
    fail_second: bool,
}

impl WorkflowHost for ReviewHost {
    fn call<'a>(
        &'a mut self,
        _tool: &'a str,
        _args: Value,
        _cancel: &'a AtomicBool,
        _deadline: Instant,
        _step_path: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        self.calls += 1;
        let result = if self.fail_second && self.calls == 2 {
            Err("second call failed".to_owned())
        } else {
            Ok(json!({"receipt":"completed-first-call", "number":self.calls}))
        };
        Box::pin(std::future::ready(result))
    }
}

fn execute(document: Value, host: &mut ReviewHost) -> workflow::WorkflowReport {
    let program = workflow::validate(&document).expect("review program validates");
    let cancel = AtomicBool::new(false);
    tauri::async_runtime::block_on(workflow::run(&program, None, host, &cancel))
}

#[test]
fn failed_iteration_keeps_completed_tool_receipt_with_iteration_path() {
    let document = json!({
        "schema_version":1,
        "inputs":{"items":[0,1]},
        "steps":[{"id":"batch","type":"for_each","items":{"$ref":"/inputs/items"},"steps":[
            {"id":"copy","type":"call","tool":"file_copy_to_ai","args":{"path":"unused"}}
        ]}],
        "outputs":{"results":{"$ref":"/steps/batch"}}
    });
    let mut host = ReviewHost { fail_second:true, ..Default::default() };
    let report = execute(document, &mut host);
    assert_eq!(report.state, "failed");
    assert_eq!(report.outputs, json!({}));
    assert_eq!(host.calls, 2);
    let trace = serde_json::to_value(&report.trace).unwrap();
    assert!(trace.as_array().unwrap().iter().any(|entry|
        entry["step_path"].as_str().unwrap_or("").contains("/iterations/0/")
            && entry["tool"] == "file_copy_to_ai"
            && entry["result"]["receipt"] == "completed-first-call"
    ));
}

#[test]
fn nested_loop_restores_outer_item_and_index() {
    let document = json!({
        "schema_version":1,
        "inputs":{"outer":["A","B"]},
        "steps":[{"id":"outer","type":"for_each","items":{"$ref":"/inputs/outer"},"steps":[
            {"id":"inner","type":"for_each","items":[10],"steps":[
                {"id":"item","type":"set","value":{"$ref":"/item"}},
                {"id":"index","type":"set","value":{"$ref":"/index"}}
            ]},
            {"id":"after","type":"set","value":{"item":{"$ref":"/item"},"index":{"$ref":"/index"}}}
        ]}],
        "outputs":{"rows":{"$ref":"/steps/outer"}}
    });
    let report = execute(document, &mut ReviewHost::default());
    assert_eq!(report.state, "succeeded", "{:?}", report.error);
    assert_eq!(report.outputs["rows"][0]["inner"][0]["item"], 10);
    assert_eq!(report.outputs["rows"][0]["after"], json!({"item":"A","index":0}));
    assert_eq!(report.outputs["rows"][1]["after"], json!({"item":"B","index":1}));
}

#[test]
fn graph_budget_stops_before_second_call() {
    let document = json!({
        "schema_version":1,
        "inputs":{},
        "limits":{"max_graph_runs":1},
        "steps":[
            {"id":"one","type":"call","tool":"graph_run","args":{}},
            {"id":"two","type":"call","tool":"graph_run","args":{}}
        ],
        "outputs":{}
    });
    let mut host = ReviewHost::default();
    let report = execute(document, &mut host);
    assert_eq!(report.state, "limited");
    assert_eq!(report.error.unwrap().code, "graph_limit");
    assert_eq!(report.graph_runs, 1);
    assert_eq!(host.calls, 1);
}
