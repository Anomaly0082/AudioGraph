use super::*;

#[derive(Default)]
struct ValueHost {
    calls: Vec<(String, Value)>,
}
impl WorkflowHost for ValueHost {
    fn call<'a>(&'a mut self, tool: &'a str, args: Value, _cancel: &'a AtomicBool,
                _deadline: Instant, _path: &'a str)
        -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        self.calls.push((tool.into(), args.clone()));
        Box::pin(std::future::ready(Ok(json!({"args": args}))))
    }
}

fn doc(steps: Value) -> Value {
    json!({"schema_version":1,"inputs":{},"steps":steps,"outputs":{}})
}
fn execute(document: &Value, overrides: Option<&Value>, host: &mut ValueHost) -> WorkflowReport {
    let workflow = validate(document).expect("valid workflow structure");
    tauri::async_runtime::block_on(run(&workflow, overrides, host, &AtomicBool::new(false)))
}
fn computation(op: &str, args: Value) -> Value {
    json!({"id":"result","type":"compute","op":op,"args":args})
}

#[test]
fn arithmetic_operations_are_traced_without_tool_calls() {
    for (op, args, expected) in [
        ("add", json!([2,3]), 5.0), ("sub", json!([2,3]), -1.0),
        ("mul", json!([-2,3]), -6.0), ("div", json!([7,2]), 3.5),
        ("abs", json!([-4]), 4.0),
    ] {
        let mut host = ValueHost::default();
        let report = execute(&doc(json!([computation(op, args)])), None, &mut host);
        assert_eq!(report.state, "succeeded", "{op}: {:?}", report.error);
        assert_eq!(report.step_results["result"].as_f64(), Some(expected));
        assert_eq!(report.steps_executed, 1);
        assert_eq!((report.tool_calls, report.graph_runs), (0,0));
        assert!(host.calls.is_empty());
        assert_eq!(report.trace.len(), 1);
        assert_eq!(report.trace[0].step_type, "compute");
        assert_eq!(report.trace[0].state, "succeeded");
        assert!(report.trace[0].tool.is_none());
    }
}

#[test]
fn nested_values_supply_graph_parameters_and_filename() {
    let mut document = doc(json!([
        {"id":"each","type":"for_each","items":[-3,2],"steps":[
            {"id":"branch","type":"if","condition":{"op":"lt","left":{"$ref":"/item"},"right":0},
             "then":[{"id":"gain","type":"compute","op":"abs","args":[{"$ref":"/item"}]}],
             "else":[{"id":"gain","type":"compute","op":"mul","args":[{"$ref":"/item"},2]}]},
            {"id":"name","type":"concat","parts":["输出_",{"$ref":"/index"},".wav"]},
            {"id":"graph","type":"call","tool":"graph_run","args":{"graph":{"nodes":[{"params":{
                "gain_db":{"$ref":"/steps/branch/steps/gain"},"output_path":{"$ref":"/steps/name"}
            }}]}}}
        ]}
    ]));
    document["outputs"] = json!({"results":{"$ref":"/steps/each"}});
    let mut host = ValueHost::default();
    let report = execute(&document, None, &mut host);
    assert_eq!(report.state, "succeeded", "{:?}", report.error);
    assert_eq!((report.tool_calls,report.graph_runs), (2,2));
    // One loop plus (if, chosen compute, concat, call) for each of two items.
    assert_eq!(report.steps_executed, 9);
    for (index, expected) in [3.0,4.0].into_iter().enumerate() {
        assert_eq!(host.calls[index].0, "graph_run");
        let params = &host.calls[index].1["graph"]["nodes"][0]["params"];
        assert_eq!(params["gain_db"].as_f64(), Some(expected));
        assert_eq!(params["output_path"], format!("输出_{index}.wav"));
    }
}

#[test]
fn computed_values_keep_existing_json_equality_and_numeric_ordering_rules() {
    for (op, right, branch) in [("eq", json!(5), "else"), ("eq", json!(5.0), "then"),
                               ("ne", json!(5), "then"), ("lte", json!(5), "then")] {
        let report = execute(&doc(json!([
            {"id":"sum","type":"compute","op":"add","args":[2,3]},
            {"id":"choice","type":"if","condition":{"op":op,"left":{"$ref":"/steps/sum"},"right":right},"then":[],"else":[]}
        ])), None, &mut ValueHost::default());
        assert_eq!(report.state,"succeeded");
        assert_eq!(report.step_results["choice"]["branch"],branch);
    }
}

#[test]
fn arithmetic_runtime_failures_keep_previous_results_and_stop_calls() {
    let mut cases = vec![
        ("add", json!(["3",1]), "invalid_type"),
        ("abs", json!([null]), "invalid_type"),
        ("mul", json!([1e308,1e308]), "numeric_overflow"),
        ("add", json!([1e308,1e308]), "numeric_overflow"),
        ("sub", json!([-1e308,1e308]), "numeric_overflow"),
        ("div", json!([1e308,1e-308]), "numeric_overflow"),
        ("add", json!([9007199254740992u64,0]), "numeric_precision"),
        ("abs", json!([-9007199254740992i64]), "numeric_precision"),
        ("abs", json!([u64::MAX]), "numeric_precision"),
        ("abs", json!([i64::MIN]), "numeric_precision"),
    ];
    for zero in [json!(0),json!(0.0),json!(-0.0)] {
        for numerator in [json!(0), json!(1), json!(-1)] {
            cases.push(("div", json!([numerator,zero]), "division_by_zero"));
        }
    }
    for (op,args,code) in cases {
        let document = doc(json!([
            {"id":"before","type":"set","value":"kept"}, computation(op,args),
            {"id":"after","type":"call","tool":"graph_run","args":{}}
        ]));
        let mut host = ValueHost::default();
        let report = execute(&document, None, &mut host);
        assert_eq!(report.error.as_ref().unwrap().code, code, "{op}: {:?}", report.error);
        assert_eq!(report.step_results["before"], "kept");
        assert!(report.step_results.get("result").is_none());
        assert_eq!(report.trace.last().unwrap().state, "failed");
        assert_eq!(report.steps_executed, 2);
        assert_eq!(report.tool_calls, 0);
        assert!(host.calls.is_empty());
    }
    for number in [json!(9007199254740991u64),json!(-9007199254740991i64)] {
        let report = execute(&doc(json!([computation("abs",json!([number]))])), None, &mut ValueHost::default());
        assert_eq!(report.state, "succeeded", "{:?}", report.error);
    }
}

#[test]
fn input_overrides_are_checked_at_execution() {
    let mut document = doc(json!([computation("add",json!([{"$ref":"/inputs/x"},1]))]));
    document["inputs"] = json!({"x":0});
    for (x, expected) in [(json!(true),"invalid_type"), (json!({}),"invalid_type"),
                          (json!([]),"invalid_type"), (json!(9007199254740992u64),"numeric_precision")] {
        let report = execute(&document,Some(&json!({"x":x})),&mut ValueHost::default());
        assert_eq!(report.error.unwrap().code, expected);
    }
}

#[test]
fn concat_accepts_only_strings_and_numbers_using_json_number_display() {
    let parts = json!(["中文",1,1.0,-2,1e20,"\"\\\n"]);
    let expected: String = parts.as_array().unwrap().iter().map(|v| match v {
        Value::String(s) => s.clone(), Value::Number(n) => n.to_string(), _ => unreachable!(),
    }).collect();
    let report = execute(&doc(json!([{"id":"result","type":"concat","parts":parts}])),None,&mut ValueHost::default());
    assert_eq!(report.state,"succeeded");
    assert_eq!(report.step_results["result"], expected);
    assert_eq!(report.trace[0].step_type,"concat");
    for invalid in [json!(null),json!(true),json!({}),json!([])] {
        let report = execute(&doc(json!([{"id":"result","type":"concat","parts":["prefix",invalid]}])),None,&mut ValueHost::default());
        assert_eq!(report.error.unwrap().code,"invalid_type");
    }
    for parts in [json!([]),json!(vec!["x";64])] {
        let report = execute(&doc(json!([{"id":"result","type":"concat","parts":parts}])),None,&mut ValueHost::default());
        assert_eq!(report.state,"succeeded");
        assert_eq!(report.tool_calls,0);
    }
    assert!(validate(&doc(json!([{"id":"result","type":"concat","parts":vec!["x";65]}]))).is_err());
}

#[test]
fn concat_limit_counts_utf8_and_json_escaping_at_exact_boundary() {
    for unit in ["a","中","\"","\\","\n","\u{0001}"] {
        let encoded_unit = bytes(&json!(unit)) - 2;
        let chunk = unit.repeat(8000);
        let chunk_bytes = encoded_unit * 8000;
        let count = (MAX_VALUE - 2) / chunk_bytes;
        let remainder = (MAX_VALUE - 2) % chunk_bytes;
        let mut parts = vec![json!({"$ref":"/inputs/chunk"}); count];
        parts.push(json!("a".repeat(remainder)));
        let mut document = doc(json!([{"id":"result","type":"concat","parts":parts}]));
        document["inputs"] = json!({"chunk":""});
        let overrides = json!({"chunk":chunk});
        let report = execute(&document,Some(&overrides),&mut ValueHost::default());
        assert_eq!(report.state,"succeeded","unit {unit:?}: {:?}",report.error);
        assert_eq!(bytes(&report.step_results["result"]),MAX_VALUE);
        document["steps"][0]["parts"].as_array_mut().unwrap().push(json!("a"));
        let report = execute(&document,Some(&overrides),&mut ValueHost::default());
        assert_eq!(report.error.unwrap().code,"value_limit","unit {unit:?}");
    }
}

#[test]
fn static_validation_checks_new_step_shape_references_and_depth() {
    for step in [
        computation("pow",json!([2,3])), computation("abs",json!([])),
        computation("abs",json!([1,2])), computation("add",json!([1])),
        computation("div",json!([1,2,3])), computation("add",json!({})),
        json!({"id":"result","type":"concat","parts":{}}),
        json!({"id":"result","type":"compute","op":"add","args":[1,2],"extra":1}),
        json!({"id":"result","type":"concat","parts":[],"extra":1}),
        computation("add",json!([{"$ref":"/steps/later"},1])),
        json!({"id":"result","type":"concat","parts":[{"$ref":"/item"}]}),
    ] {
        assert!(validate(&doc(json!([step]))).is_err(),"accepted {step}");
    }
    let mut deep = json!(0);
    for _ in 0..MAX_DEPTH+1 { deep = json!([deep]); }
    assert_eq!(validate(&doc(json!([computation("abs",json!([deep]))]))).unwrap_err().code,"depth_limit");
}

#[test]
fn ordinary_objects_and_literal_expressions_keep_existing_semantics() {
    let data = json!({"$compute":{"op":"add","args":[1,2]},"$calc":"data","$concat":["a","b"]});
    let document = doc(json!([
        {"id":"data","type":"set","value":data},
        {"id":"literal","type":"set","value":{"$literal":{"$ref":"/not/a/reference"}}},
        {"id":"number","type":"compute","op":"add","args":[{"$literal":2},3]},
        {"id":"string","type":"concat","parts":[{"$literal":"ok"},{"$ref":"/steps/number"}]}
    ]));
    let report = execute(&document,None,&mut ValueHost::default());
    assert_eq!(report.state,"succeeded");
    assert_eq!(report.step_results["data"],data);
    assert_eq!(report.step_results["literal"],json!({"$ref":"/not/a/reference"}));
}

#[test]
fn unselected_branch_does_not_evaluate_new_steps() {
    let document = doc(json!([{"id":"choice","type":"if","condition":{"op":"eq","left":1,"right":1},
        "then":[{"id":"ok","type":"concat","parts":[]}],
        "else":[computation("div",json!([1,0])),{"id":"bad","type":"concat","parts":[null]}]}]));
    let report = execute(&document,None,&mut ValueHost::default());
    assert_eq!(report.state,"succeeded");
    assert_eq!(report.steps_executed,2);
    assert_eq!(report.trace.len(),2);
}

#[test]
fn new_steps_obey_step_budget_and_cancel_before_execution() {
    let mut document = doc(json!([computation("add",json!([1,2])),{"id":"next","type":"concat","parts":[]} ]));
    document["limits"] = json!({"max_steps":1,"max_tool_calls":0,"max_graph_runs":0});
    let report = execute(&document,None,&mut ValueHost::default());
    assert_eq!(report.error.unwrap().code,"step_limit");
    assert_eq!(report.steps_executed,1);
    assert_eq!(report.step_results["result"].as_f64(),Some(3.0));
    let workflow = validate(&document).unwrap();
    let mut host = ValueHost::default();
    let report = tauri::async_runtime::block_on(run(&workflow,None,&mut host,&AtomicBool::new(true)));
    assert_eq!(report.state,"cancelled");
    assert_eq!(report.steps_executed,0);
    assert!(host.calls.is_empty());
}

#[test]
fn expired_deadline_stops_value_steps_before_evaluation() {
    for step in [computation("add",json!([1,2])),json!({"id":"result","type":"concat","parts":[]})] {
        let workflow = validate(&doc(json!([step]))).unwrap();
        let mut host = ValueHost::default();
        let cancel = AtomicBool::new(false);
        let mut runner = Runner {
            host: &mut host, cancel: &cancel, deadline: Instant::now(),
            limits: &workflow.limits, report: WorkflowReport::empty(),
        };
        let mut context = Context { inputs: json!({}), frames: vec![], item: None, index: None };
        let error = tauri::async_runtime::block_on(runner.run_steps(&workflow.steps,&mut context,"/steps".into())).unwrap_err();
        assert_eq!(error.code,"timeout");
        assert_eq!(runner.report.steps_executed,0);
        assert_eq!(runner.report.trace[0].state,"failed");
        assert_eq!(runner.report.tool_calls,0);
    }
}
