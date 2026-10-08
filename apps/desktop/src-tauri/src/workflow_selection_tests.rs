use super::*;

#[derive(Default)]
struct SelectionHost { calls: Vec<(String, Value)> }
impl WorkflowHost for SelectionHost {
    fn call<'a>(&'a mut self, tool: &'a str, args: Value, _cancel: &'a AtomicBool,
        _deadline: Instant, _path: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        self.calls.push((tool.to_owned(), args));
        Box::pin(std::future::ready(Ok(json!({"ok":true}))))
    }
}
fn selection_doc(items: Value, steps: Value) -> Value {
    json!({"schema_version":1,"inputs":{"items":items},"steps":steps,
        "outputs":{"result":{"$ref":"/steps/result"}}})
}
fn choose(order: &str) -> Value {
    json!({"id":"result","type":"select","items":{"$ref":"/inputs/items"},
        "by":{"$ref":"/item/score"},"order":order})
}
fn selection_run(doc: &Value, host: &mut SelectionHost) -> WorkflowReport {
    let workflow = validate(doc).unwrap();
    tauri::async_runtime::block_on(run(&workflow, None, host, &AtomicBool::new(false)))
}
fn selection_error(doc: &Value, code: &str) {
    let report = selection_run(doc, &mut SelectionHost::default());
    assert_eq!(report.error.as_ref().map(|e| e.code.as_str()), Some(code), "{report:?}");
}

#[test]
fn selection_filter_then_select_exports_only_the_complete_chosen_item() {
    let candidates = json!([
        {"path":"clipped.wav","params":{"gain":9},"clip":1,"score":0},
        {"path":"first.wav","params":{"gain":-3},"clip":0,"score":3},
        {"path":"best.wav","params":{"gain":2,"nested":[1,2]},"clip":0,"score":1}
    ]);
    let mut doc = selection_doc(candidates.clone(), json!([
        {"id":"filtered","type":"filter","items":{"$ref":"/inputs/items"},
            "condition":{"op":"lte","left":{"$ref":"/item/clip"},"right":0}},
        {"id":"chosen","type":"select","items":{"$ref":"/steps/filtered"},
            "by":{"$ref":"/item/score"},"order":"min"},
        {"id":"result","type":"if","condition":{"op":"eq","left":{"$ref":"/steps/chosen/found"},"right":true},
            "then":[{"id":"export","type":"call","tool":"file_export","args":{
                "path":{"$ref":"/steps/chosen/item/path"},"params":{"$ref":"/steps/chosen/item/params"}}}],"else":[]}
    ]));
    let mut host = SelectionHost::default();
    let report = selection_run(&doc, &mut host);
    assert_eq!(report.state, "succeeded", "{:?}", report.error);
    assert_eq!(report.step_results["filtered"], json!([candidates[1],candidates[2]]));
    assert_eq!(report.step_results["chosen"], json!({"found":true,"item":candidates[2],"index":1,"score":1}));
    assert!(report.step_results["chosen"]["score"].as_number().unwrap().is_i64());
    assert_eq!(host.calls, vec![("file_export".into(),json!({"path":"best.wav","params":candidates[2]["params"]}))]);
    doc["inputs"]["items"] = json!([{ "clip":1,"score":1 }]);
    let mut host = SelectionHost::default();
    let report = selection_run(&doc, &mut host);
    assert_eq!(report.step_results["filtered"], json!([]));
    assert_eq!(report.step_results["chosen"], json!({"found":false,"item":null,"index":null,"score":null}));
    assert_eq!(report.outputs["result"]["branch"], "else");
    assert!(host.calls.is_empty());
}

#[test]
fn selection_min_max_ties_empty_and_original_indices() {
    let items = json!([{"score":3,"extra":"first"},{"score":1},{"score":1,"extra":"tie"},{"score":3}]);
    for (order,index) in [("min",1),("max",0)] {
        let report = selection_run(&selection_doc(items.clone(),json!([choose(order)])), &mut SelectionHost::default());
        assert_eq!(report.outputs["result"],json!({"found":true,"item":items[index],"index":index,"score":items[index]["score"]}));
        assert_eq!((report.steps_executed,report.tool_calls,report.graph_runs),(1,0,0));
    }
    let report = selection_run(&selection_doc(json!([]),json!([choose("min")])), &mut SelectionHost::default());
    assert_eq!(report.outputs["result"],json!({"found":false,"item":null,"index":null,"score":null}));
}

#[test]
fn selection_checks_every_score_including_a_losing_last_item() {
    for bad in [json!("1"),json!(null),json!(true),json!([]),json!({})] {
        selection_error(&selection_doc(json!([{"score":0},{"score":bad}]),json!([choose("min")])),"invalid_type");
    }
    for bad in [json!(9007199254740992u64),json!(-9007199254740992i64)] {
        selection_error(&selection_doc(json!([{"score":0},{"score":bad}]),json!([choose("min")])),"numeric_precision");
    }
    selection_error(&selection_doc(json!([{"score":0},{}]),json!([choose("min")])),"missing_ref");
}

#[test]
fn selection_filter_comparisons_match_if_and_preserve_order() {
    for (op,right,expected) in [("eq",json!(2),json!([2])),("ne",json!(2),json!([1,3])),
        ("lt",json!(2),json!([1])),("lte",json!(2),json!([1,2])),
        ("gt",json!(2),json!([3])),("gte",json!(2),json!([2,3]))] {
        let doc = selection_doc(json!([1,2,3]),json!([{"id":"result","type":"filter","items":{"$ref":"/inputs/items"},
            "condition":{"op":op,"left":{"$ref":"/item"},"right":right}}]));
        let report = selection_run(&doc, &mut SelectionHost::default());
        assert_eq!(report.outputs["result"],expected);
        assert_eq!(report.steps_executed,1);
    }
    let steps = json!([{"id":"result","type":"filter","items":{"$ref":"/inputs/items"},
        "condition":{"op":"lt","left":{"$ref":"/item/score"},"right":2}}]);
    selection_error(&selection_doc(json!([{"score":1},{}]),steps.clone()),"missing_ref");
    selection_error(&selection_doc(json!([{"score":"1"}]),steps),"invalid_type");
}

#[test]
fn selection_capacity_and_step_budgets() {
    for kind in ["filter","select"] {
        let step = if kind == "select" { choose("min") } else {
            json!({"id":"result","type":"filter","items":{"$ref":"/inputs/items"},"condition":{"op":"eq","left":1,"right":1}})
        };
        selection_error(&selection_doc(json!({}),json!([step.clone()])),"invalid_type");
        selection_error(&selection_doc(json!(vec![json!({"score":1});17]),json!([step.clone()])),"item_limit");
        let report = selection_run(&selection_doc(json!(vec![json!({"score":1});16]),json!([step.clone()])), &mut SelectionHost::default());
        assert_eq!(report.state,"succeeded");
        let mut doc = selection_doc(json!([]),json!([step]));
        doc["limits"] = json!({"max_steps":0});
        selection_error(&doc,"step_limit");
    }
}

#[test]
fn selection_nested_loop_restores_outer_bindings_and_evaluates_items_outside() {
    let doc = selection_doc(json!([{ "score":42,"children":[{"score":3},{"score":1}] }]),json!([
        {"id":"result","type":"for_each","items":{"$ref":"/inputs/items"},"steps":[
            {"id":"filtered","type":"filter","items":{"$ref":"/item/children"},"condition":{"op":"gte","left":{"$ref":"/item/score"},"right":0}},
            {"id":"chosen","type":"select","items":{"$ref":"/item/children"},"by":{"$ref":"/item/score"},"order":"min"},
            {"id":"outer","type":"set","value":{"score":{"$ref":"/item/score"},"index":{"$ref":"/index"}}}
        ]}
    ]));
    let report = selection_run(&doc,&mut SelectionHost::default());
    assert_eq!(report.state,"succeeded","{:?}",report.error);
    assert_eq!(report.outputs["result"][0]["outer"],json!({"score":42,"index":0}));
    assert_eq!(report.outputs["result"][0]["chosen"]["index"],1);
}

#[test]
fn selection_static_shape_and_scope_are_checked() {
    for (key,value) in [("order",json!("ascending")),("unexpected",json!(true))] {
        let mut step = choose("min"); step[key] = value;
        assert!(validate(&selection_doc(json!([]),json!([step]))).is_err());
    }
    let mut step = choose("min"); step.as_object_mut().unwrap().remove("by");
    assert!(validate(&selection_doc(json!([]),json!([step]))).is_err());
    let mut step = choose("min"); step["items"] = json!({"$ref":"/item"});
    assert!(validate(&selection_doc(json!([]),json!([step]))).is_err());
    let mut doc = selection_doc(json!([]),json!([choose("min")]));
    doc["outputs"] = json!({"leaked":{"$ref":"/item"}});
    assert!(validate(&doc).is_err());
    let mut deep = json!(0); for _ in 0..40 { deep = json!([deep]); }
    let error = validate(&selection_doc(deep,json!([choose("min")]))).unwrap_err();
    assert_eq!(error.code,"depth_limit");
}

#[test]
fn selection_pre_cancelled_workflow_never_calls_host() {
    let workflow = validate(&selection_doc(json!([{"score":1}]),json!([choose("min")]))).unwrap();
    let mut host = SelectionHost::default();
    let report = tauri::async_runtime::block_on(run(&workflow,None,&mut host,&AtomicBool::new(true)));
    assert_eq!(report.state,"cancelled");
    assert!(host.calls.is_empty());
}

#[test]
fn selection_error_and_timeout_restore_the_outer_context() {
    for kind in ["filter","select"] {
        let step = if kind == "select" { choose("min") } else {
            json!({"id":"result","type":"filter","items":{"$ref":"/inputs/items"},
                "condition":{"op":"lte","left":{"$ref":"/item/score"},"right":0}})
        };
        let workflow = validate(&selection_doc(json!([{"score":0},{}]),json!([step]))).unwrap();
        let mut host = SelectionHost::default();
        let cancel = AtomicBool::new(false);
        let mut runner = Runner { host:&mut host,cancel:&cancel,deadline:Instant::now()+Duration::from_secs(10),
            limits:&workflow.limits,report:WorkflowReport::empty() };
        let outer = json!({"outer":true});
        let mut ctx = Context { inputs:Value::Object(workflow.inputs.clone()),frames:vec![Map::new()],item:Some(outer.clone()),index:Some(7) };
        let error = tauri::async_runtime::block_on(runner.run_step(&workflow.steps[0],&mut ctx,"/steps/0")).unwrap_err();
        assert_eq!(error.code,"missing_ref");
        assert_eq!(ctx.item,Some(outer.clone()));
        assert_eq!(ctx.index,Some(7));
        runner.deadline = Instant::now()-Duration::from_secs(1);
        let error = tauri::async_runtime::block_on(runner.run_step(&workflow.steps[0],&mut ctx,"/steps/0")).unwrap_err();
        assert_eq!(error.code,"timeout");
        assert_eq!(ctx.item,Some(outer));
        assert_eq!(ctx.index,Some(7));
    }
}

#[test]
fn selection_result_wrapper_and_total_state_obey_existing_limits() {
    let workflow = validate(&selection_doc(json!([]),json!([choose("min")]))).unwrap();
    // The input array fits, but the selected-item wrapper makes the result too large.
    let item = json!({"score":0,"data":"x".repeat(MAX_VALUE-60)});
    let inputs = json!({"items":[item]});
    assert!(bytes(&inputs["items"])<=MAX_VALUE);
    let mut host = SelectionHost::default();
    let report = tauri::async_runtime::block_on(run(&workflow,Some(&inputs),&mut host,&AtomicBool::new(false)));
    assert_eq!(report.error.as_ref().map(|e|e.code.as_str()),Some("value_limit"));

    let mut steps = Vec::new();
    for index in 0..6 {
        steps.push(json!({"id":format!("choice{index}"),"type":"select","items":{"$ref":"/inputs/items"},
            "by":{"$ref":"/item/score"},"order":"min"}));
    }
    let mut doc = selection_doc(json!([]),Value::Array(steps));
    doc["outputs"] = json!({});
    let workflow = validate(&doc).unwrap();
    let inputs = json!({"items":[{"score":0,"data":"x".repeat(200_000)}]});
    let report = tauri::async_runtime::block_on(run(&workflow,Some(&inputs),&mut host,&AtomicBool::new(false)));
    assert_eq!(report.error.as_ref().map(|e|e.code.as_str()),Some("state_limit"));
}
