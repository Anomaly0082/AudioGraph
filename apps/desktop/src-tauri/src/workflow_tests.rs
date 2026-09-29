use super::*;
use std::sync::atomic::AtomicBool;

#[derive(Default)]
struct MockHost {
    calls: Vec<(String, Value)>,
    fail_at: Option<usize>,
    cancel_at: Option<usize>,
    huge_result: bool,
}
impl WorkflowHost for MockHost {
    fn call<'a>(
        &'a mut self, tool: &'a str, args: Value, cancel: &'a AtomicBool, _deadline: Instant, _step_path: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        self.calls.push((tool.into(), args.clone()));
        let n = self.calls.len();
        if self.cancel_at == Some(n) { cancel.store(true, Ordering::Release); }
        let result = if self.fail_at == Some(n) {
            Err("mock tool failure".into())
        } else if self.huge_result {
            Ok(json!({"data": "x".repeat(MAX_VALUE)}))
        } else {
            Ok(json!({"call": n, "args": args}))
        };
        Box::pin(std::future::ready(result))
    }
}

fn document(steps: Value, outputs: Value) -> Value {
    json!({"schema_version":1,"inputs":{},"steps":steps,"outputs":outputs})
}
fn run_test(doc: &Value, host: &mut MockHost) -> WorkflowReport {
    let workflow = validate(doc).unwrap();
    let cancel = AtomicBool::new(false);
    tauri::async_runtime::block_on(run(&workflow, None, host, &cancel))
}

#[test]
fn gain_loop_calls_graph_exactly_three_times_and_collects_real_results() {
    let doc = json!({
        "schema_version":1,
        "inputs":{"gains":[
            {"gain_db":-3,"path":"minus.wav"},
            {"gain_db":0,"path":"flat.wav"},
            {"gain_db":3,"path":"plus.wav"}
        ]},
        "steps":[
            {"id":"each","type":"for_each","items":{"$ref":"/inputs/gains"},"steps":[
                {"id":"graph","type":"call","tool":"graph_run",
                 "args":{"graph":{"nodes":[{"params":{
                     "gain_db":{"$ref":"/item/gain_db"},
                     "output_path":{"$ref":"/item/path"}
                 }}]}}},
                {"id":"index","type":"set","value":{"$ref":"/index"}}
            ]}
        ],
        "outputs":{"results":{"$ref":"/steps/each"}}
    });
    let mut host = MockHost::default();
    let report = run_test(&doc, &mut host);
    assert_eq!(report.state, "succeeded", "{:?}", report.error);
    assert_eq!(report.graph_runs, 3);
    assert_eq!(host.calls.len(), 3);
    assert_eq!(report.outputs["results"][0]["graph"]["call"], 1);
    assert_eq!(report.outputs["results"][1]["graph"]["call"], 2);
    assert_eq!(report.outputs["results"][2]["graph"]["call"], 3);
    assert_eq!(report.outputs["results"][2]["index"], 2);
    assert_eq!(host.calls[0].1["graph"]["nodes"][0]["params"]["gain_db"], -3);
    assert_eq!(host.calls[2].1["graph"]["nodes"][0]["params"]["output_path"], "plus.wav");
}

#[test]
fn refs_are_data_and_literal_escapes_expression() {
    let doc = json!({"schema_version":1,"inputs":{"data":{"$ref":"/nothing"}},
        "steps":[
            {"id":"a","type":"set","value":{"$ref":"/inputs/data"}},
            {"id":"b","type":"set","value":{"$literal":{"$ref":"/steps/a"}}}
        ],"outputs":{"a":{"$ref":"/steps/a"},"b":{"$ref":"/steps/b"}}});
    let report = run_test(&doc, &mut MockHost::default());
    assert_eq!(report.state, "succeeded");
    assert_eq!(report.outputs["a"], json!({"$ref":"/nothing"}));
    assert_eq!(report.outputs["b"], json!({"$ref":"/steps/a"}));
}

#[test]
fn both_branches_execute_correctly_and_locals_do_not_leak() {
    let base = document(
        json!([{"id":"choice","type":"if","condition":{
            "op":"lt","left":{"$ref":"/inputs/x"},"right":5
        },"then":[{"id":"selected","type":"set","value":"small"}],
          "else":[{"id":"selected","type":"set","value":"large"}]}]),
        json!({"result":{"$ref":"/steps/choice"}}),
    );
    let mut doc = base.clone();
    doc["inputs"] = json!({"x":2});
    let report = run_test(&doc, &mut MockHost::default());
    assert_eq!(report.outputs["result"]["branch"], "then");
    assert_eq!(report.outputs["result"]["steps"]["selected"], "small");
    doc["inputs"] = json!({"x":9});
    let report = run_test(&doc, &mut MockHost::default());
    assert_eq!(report.outputs["result"]["branch"], "else");
    assert_eq!(report.outputs["result"]["steps"]["selected"], "large");
    doc["outputs"] = json!({"bad":{"$ref":"/steps/selected"}});
    assert_eq!(validate(&doc).unwrap_err().code, "invalid_ref");
}

#[test]
fn static_validation_rejects_fields_tools_forward_refs_and_shadowing() {
    let cases = [
        (document(json!([{"id":"a","type":"set","value":1,"extra":true}]), json!({})), "unknown_field"),
        (document(json!([{"id":"a","type":"call","tool":"workflow_run","args":{}}]), json!({})), "unknown_tool"),
        (document(json!([{"id":"a","type":"set","value":{"$ref":"/steps/b"}},
                         {"id":"b","type":"set","value":1}]), json!({})), "invalid_ref"),
        (document(json!([{"id":"a","type":"set","value":1},
                         {"id":"loop","type":"for_each","items":[],
                          "steps":[{"id":"a","type":"set","value":2}]}]), json!({})), "duplicate_id"),
        (document(json!([{"id":"a","type":"if","condition":{"op":"eq","left":1,"right":1},
                         "then":[],"else":[{"id":"bad","type":"set","value":{"$ref":"/steps/later"}}]},
                        {"id":"later","type":"set","value":1}]), json!({})), "invalid_ref"),
    ];
    for (doc, expected) in cases {
        assert_eq!(validate(&doc).unwrap_err().code, expected);
    }
}

#[test]
fn limits_types_and_input_override_are_enforced() {
    let doc = document(json!([{"id":"loop","type":"for_each","items":17,"steps":[]}]), json!({}));
    assert_eq!(run_test(&doc, &mut MockHost::default()).error.unwrap().code, "invalid_type");
    let doc = document(json!([{"id":"loop","type":"for_each","items":(0..17).collect::<Vec<_>>(),"steps":[]}]), json!({}));
    assert_eq!(run_test(&doc, &mut MockHost::default()).state, "limited");
    let mut doc = document(json!([{"id":"a","type":"set","value":1},{"id":"b","type":"set","value":2}]), json!({}));
    doc["limits"] = json!({"max_steps":1});
    let report = run_test(&doc, &mut MockHost::default());
    assert_eq!(report.state, "limited");
    assert_eq!(report.steps_executed, 1);
    assert_eq!(report.step_results["a"], 1);
    assert!(report.step_results.get("b").is_none());
    assert_eq!(validate(&json!({"schema_version":1,"inputs":{},"steps":[],"outputs":{},"limits":{"max_steps":257}})).unwrap_err().code, "invalid_limit");
    let workflow = validate(&document(json!([]), json!({}))).unwrap();
    let cancel = AtomicBool::new(false);
    let report = tauri::async_runtime::block_on(run(&workflow, Some(&json!({"undeclared":1})), &mut MockHost::default(), &cancel));
    assert_eq!(report.error.unwrap().code, "unknown_input");
}

#[test]
fn tool_failure_and_cancel_stop_later_steps_with_partial_report() {
    let doc = document(json!([
        {"id":"before","type":"set","value":"kept"},
        {"id":"first","type":"call","tool":"file_read_text","args":{}},
        {"id":"after","type":"call","tool":"file_read_text","args":{}}
    ]), json!({"done":true}));
    let mut host = MockHost { fail_at: Some(1), ..Default::default() };
    let report = run_test(&doc, &mut host);
    assert_eq!(report.state, "failed");
    assert_eq!(report.outputs, json!({}));
    assert_eq!(report.step_results["before"], "kept");
    assert_eq!(report.error.unwrap().code, "tool_failed");
    assert_eq!(host.calls.len(), 1);

    let mut host = MockHost { cancel_at: Some(1), ..Default::default() };
    let report = run_test(&doc, &mut host);
    assert_eq!(report.state, "cancelled");
    assert_eq!(host.calls.len(), 1);
}

#[test]
fn completed_nested_call_remains_in_trace_after_later_failure() {
    let doc = document(json!([
        {"id":"each","type":"for_each","items":[1,2],"steps":[
            {"id":"tool","type":"call","tool":"graph_run","args":{"n":{"$ref":"/item"}}},
            {"id":"must_fail","type":"set","value":{"$ref":"/item/missing"}}
        ]},
        {"id":"after","type":"set","value":true}
    ]), json!({}));
    let mut host = MockHost::default();
    let report = run_test(&doc, &mut host);
    assert_eq!(report.state, "failed");
    assert_eq!(host.calls.len(), 1);
    assert_eq!(report.trace[0].step_path, "/steps/0:each/iterations/0/0:tool");
    assert_eq!(report.trace[0].tool.as_deref(), Some("graph_run"));
    assert_eq!(report.trace[0].result.as_ref().unwrap()["call"], 1);
    assert_eq!(report.error.unwrap().step_path, "/steps/0:each/iterations/0/1:must_fail");
}

#[test]
fn completed_nested_set_remains_in_trace_after_later_failure() {
    let doc = document(json!([
        {"id":"choice","type":"if",
         "condition":{"op":"eq","left":1,"right":1},
         "then":[
             {"id":"local","type":"set","value":{"kept":true}},
             {"id":"broken","type":"set","value":{"$ref":"/inputs/missing"}}
         ]}
    ]), json!({}));
    // A statically invalid reference is rejected even in an unselected branch.
    assert_eq!(validate(&doc).unwrap_err().code, "invalid_ref");
    let doc = json!({"schema_version":1,"inputs":{"x":{}},
        "steps":[{"id":"choice","type":"if",
            "condition":{"op":"eq","left":1,"right":1},
            "then":[
                {"id":"local","type":"set","value":{"kept":true}},
                {"id":"broken","type":"set","value":{"$ref":"/inputs/x/missing"}}
            ]}],
        "outputs":{}});
    let report = run_test(&doc, &mut MockHost::default());
    assert_eq!(report.state, "failed");
    assert_eq!(report.trace[0].id, "local");
    assert_eq!(report.trace[0].result.as_ref().unwrap(), &json!({"kept":true}));
}

#[test]
fn oversized_tool_result_is_explicit_limit_failure() {
    let doc = document(json!([{"id":"big","type":"call","tool":"file_read_text","args":{}}]), json!({}));
    let mut host = MockHost { huge_result: true, ..Default::default() };
    let report = run_test(&doc, &mut host);
    assert_eq!(report.state, "limited");
    assert_eq!(report.error.unwrap().code, "value_limit");
}

#[test]
fn ordered_comparison_requires_numbers() {
    let mut doc = document(json!([{"id":"choice","type":"if",
        "condition":{"op":"lt","left":"a","right":2},"then":[],"else":[]}]), json!({}));
    assert_eq!(run_test(&doc, &mut MockHost::default()).error.unwrap().code, "invalid_type");
    doc["steps"][0]["condition"]["op"] = json!("eq");
    assert_eq!(run_test(&doc, &mut MockHost::default()).state, "succeeded");
}
