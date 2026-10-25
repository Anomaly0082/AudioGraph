use super::*;

struct RuntimeHost { result: Value, calls: usize }
impl WorkflowHost for RuntimeHost {
    fn call<'a>(&'a mut self, _: &'a str, _: Value, _: &'a AtomicBool, _: Instant, _: &'a str)
        -> Pin<Box<dyn Future<Output=Result<Value,String>>+Send+'a>> {
        self.calls += 1;
        Box::pin(std::future::ready(Ok(self.result.clone())))
    }
}
fn nested(n: usize) -> Value { (0..n).fold(json!(0), |value,_| json!([value])) }
fn chain(n: usize) -> Vec<Value> {
    let mut steps = vec![json!({"id":"s0","type":"set","value":0})];
    for i in 1..=n {
        steps.push(json!({"id":format!("s{i}"),"type":"set","value":{"wrap":{"$ref":format!("/steps/s{}",i-1)}}}));
    }
    steps
}
fn call(id: &str) -> Value { json!({"id":id,"type":"call","tool":"nodes_list","args":{}}) }
fn program(steps: Vec<Value>, outputs: Value) -> Workflow {
    validate(&json!({"schema_version":1,"inputs":{},"steps":steps,"outputs":outputs})).unwrap()
}
fn execute(workflow: &Workflow, host: &mut RuntimeHost) -> WorkflowReport {
    tauri::async_runtime::block_on(run(workflow,None,host,&AtomicBool::new(false)))
}
fn limited(report: &WorkflowReport) {
    assert_eq!(report.state,"limited","{report:?}");
    assert_eq!(report.error.as_ref().map(|error| error.code.as_str()),Some("depth_limit"));
    assert_eq!(report.outputs,json!({}));
}

#[test]
fn shallow_references_cannot_accumulate_past_runtime_depth() {
    let mut steps = chain(40); steps.push(call("never"));
    let mut host = RuntimeHost {result:json!({}),calls:0};
    let report = execute(&program(steps,json!({"value":{"$ref":"/steps/s40"}})),&mut host);
    limited(&report);
    assert_eq!(report.steps_executed,34);
    assert_eq!(host.calls,0);
    assert!(report.step_results.get("s32").is_some());
    assert!(report.step_results.get("s33").is_none());
    assert_eq!(report.trace.last().unwrap().state,"failed");
}

#[test]
fn depth_32_is_valid_but_final_output_wrapper_must_also_fit() {
    let mut host = RuntimeHost {result:json!({}),calls:0};
    let report = execute(&program(chain(32),json!({})),&mut host);
    assert_eq!(report.state,"succeeded","{:?}",report.error);
    depth(&report.step_results["s32"],0,"test").unwrap();
    let report = execute(&program(chain(32),json!({"value":{"$ref":"/steps/s32"}})),&mut host);
    limited(&report);
    assert!(report.error.as_ref().unwrap().step_path.starts_with("/outputs/"));
}

#[test]
fn host_result_depth_is_checked_before_binding_or_next_call() {
    for (n,expected) in [(31,"succeeded"),(32,"limited")] {
        let mut host = RuntimeHost {result:json!({"payload":nested(n)}),calls:0};
        let report = execute(&program(vec![call("first"),call("second")],json!({})),&mut host);
        assert_eq!(report.state,expected,"{report:?}");
        if expected == "limited" {
            limited(&report); assert_eq!(host.calls,1); assert_eq!(report.tool_calls,1);
            assert!(report.step_results.get("first").is_none());
            assert_eq!(report.trace[0].state,"failed");
        } else { assert_eq!(host.calls,2); }
    }
}

fn wrapper(kind: &str) -> Value {
    let child = json!({"id":"copy","type":"set","value":{"$ref":"/steps/deep"}});
    if kind == "for_each" { json!({"id":"wrapped","type":"for_each","items":[0],"steps":[child]}) }
    else { json!({"id":"wrapped","type":"if","condition":{"op":"eq","left":1,"right":1},"then":[child]}) }
}
#[test]
fn loop_and_branch_wrappers_limit_depth_and_stop_following_tools() {
    for kind in ["for_each","if"] {
        let mut steps = chain(31);
        steps.push(json!({"id":"deep","type":"set","value":{"$ref":"/steps/s31"}}));
        steps.push(wrapper(kind)); steps.push(call("never"));
        let mut host = RuntimeHost {result:json!({}),calls:0};
        let report = execute(&program(steps,json!({})),&mut host);
        limited(&report); assert_eq!(host.calls,0);
        assert!(report.step_results.get("wrapped").is_none());
        assert_eq!(report.trace.last().unwrap().state,"failed");
    }
}

#[test]
fn wrapper_failure_restores_outer_loop_bindings() {
    for kind in ["for_each","if"] {
        for n in [30,31] {
            let workflow = program(vec![json!({"id":"deep","type":"set","value":0}),wrapper(kind)],json!({}));
            let mut host = RuntimeHost {result:json!({}),calls:0}; let cancel = AtomicBool::new(false);
            let mut runner = Runner {host:&mut host,cancel:&cancel,deadline:Instant::now()+Duration::from_secs(1),limits:&workflow.limits,report:WorkflowReport::empty()};
            let outer = json!({"outer":true});
            let mut frame = Map::new(); frame.insert("deep".into(),nested(n));
            let mut ctx = Context {inputs:json!({}),frames:vec![frame],item:Some(outer.clone()),index:Some(7)};
            let result = tauri::async_runtime::block_on(runner.run_step(&workflow.steps[1],&mut ctx,"/steps/wrapped"));
            if n == 30 { assert!(result.is_ok(),"{result:?}"); }
            else { assert_eq!(result.unwrap_err().code,"depth_limit"); }
            assert_eq!(ctx.item,Some(outer)); assert_eq!(ctx.index,Some(7));
            assert_eq!(ctx.frames.len(),1); assert!(!ctx.frames[0].contains_key("copy"));
        }
    }
}

#[test]
fn loop_checks_each_collected_wrapper_before_starting_next_iteration() {
    let step = json!({"id":"batch","type":"for_each","items":[0,1],"steps":[call("child")]});
    let mut host = RuntimeHost {result:json!({"payload":nested(30)}),calls:0};
    let report = execute(&program(vec![step,call("never")],json!({})),&mut host);
    limited(&report); assert_eq!(host.calls,1); assert_eq!(report.tool_calls,1);
    assert_eq!(report.trace[0].state,"succeeded");
    assert_eq!(report.trace.last().unwrap().state,"failed");
}
