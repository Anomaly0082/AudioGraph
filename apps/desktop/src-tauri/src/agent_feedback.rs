//! Model-facing execution receipts. Full results remain in events and the run store.
use serde_json::{json, Value};

const MAX_INLINE_RESULT: usize = 12 * 1024;

fn small(value: &Value) -> Value {
    if serde_json::to_vec(value).map_or(usize::MAX, |v| v.len()) <= 1024 {
        value.clone()
    } else {
        json!({"complete":false,"preview":super::bounded_text(&value.to_string(),512)})
    }
}

pub(super) fn model_receipt(name: &str, envelope: &Value) -> Value {
    if !matches!(name, "graph_run" | "workflow_run")
        || serde_json::to_vec(envelope).map_or(usize::MAX, |v| v.len()) <= MAX_INLINE_RESULT {
        return envelope.clone();
    }
    let mut receipt = json!({"ok":envelope["ok"],"complete":false,"data":{},
        "notice":"Execution report omitted from model context because of size. This is only a receipt, not a complete result or configuration. Do not invent omitted outputs. Full returned data remains in local tool details. If a run_id is present, query runs_read for the relevant result pointer; record_warning or recording_warning means stored results may be incomplete."});
    if let Some(error) = envelope.get("error") { receipt["error"] = small(error); }
    if let Some(data) = envelope.get("data").and_then(Value::as_object) {
        for key in ["schema_version", "state", "run_id", "steps_executed", "tool_calls", "graph_runs", "error", "record_warning", "recording_warning"] {
            if let Some(value) = data.get(key) { receipt["data"][key] = small(value); }
        }
        if let Some(id) = data.get("run_id").and_then(Value::as_str)
            .filter(|id| id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())) {
            receipt["read_result"] = json!({"tool":"runs_read","args":{"id":id,"section":"result"}});
        }
    }
    // Legacy Graph failures attach host record metadata at the end of a string.
    // Keep it even when a long backend error's middle/body is omitted.
    if let Some(error) = envelope.get("error").and_then(Value::as_str) {
        if let Some((_, suffix)) = error.rsplit_once("; run_id=") {
            if let Some(id) = suffix.get(..64).filter(|id| id.bytes().all(|b| b.is_ascii_hexdigit())) {
                receipt["data"]["run_id"] = json!(id);
                receipt["read_result"] = json!({"tool":"runs_read","args":{"id":id,"section":"result"}});
            }
        }
        if let Some((_, warning)) = error.rsplit_once("; run record warning:") {
            receipt["data"]["record_warning"] = small(&json!(warning.trim()));
        }
    }
    receipt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_execution_receipt_preserves_truth_and_retrieval_without_mutating_full_report() {
        for state in ["succeeded", "failed", "cancelled"] {
            let envelope = json!({"ok":state=="succeeded","error":{"code":"tool_failed","message":"partial failure"},
                "data":{"state":state,"run_id":"a".repeat(64),"graph_runs":2,"steps_executed":4,
                    "record_warning":"execution record incomplete", "recording_warning":"cannot finish record", "error":{"step_path":"/steps/2","message":"failed"},
                    "outputs":{"large":"x".repeat(160*1024)}}});
            let receipt = model_receipt("workflow_run", &envelope);
            assert_eq!(receipt["ok"], envelope["ok"]);
            assert_eq!(receipt["complete"], false);
            assert_eq!(receipt["data"]["state"], state);
            assert_eq!(receipt["data"]["error"], envelope["data"]["error"]);
            assert_eq!(receipt["data"]["recording_warning"], "cannot finish record");
            assert_eq!(receipt["data"]["record_warning"], "execution record incomplete");
            assert_eq!(receipt["read_result"]["args"]["id"], "a".repeat(64));
            assert!(receipt["data"].get("outputs").is_none());
            assert!(serde_json::to_vec(&receipt).unwrap().len() < MAX_INLINE_RESULT);
            assert_eq!(envelope["data"]["outputs"]["large"].as_str().unwrap().len(),160*1024);
        }
    }

    #[test]
    fn normal_results_and_non_execution_tools_are_unchanged() {
        let small = json!({"ok":true,"data":{"state":"succeeded","outputs":{"peak":0.5}}});
        assert_eq!(model_receipt("graph_run", &small), small);
        let text = json!({"ok":true,"data":{"text":"x".repeat(16*1024)}});
        assert_eq!(model_receipt("file_read_text", &text), text);
        let error = json!({"ok":false,"error":"e".repeat(20*1024)});
        let receipt = model_receipt("graph_run", &error);
        assert_eq!(receipt["ok"], false);
        assert_eq!(receipt["error"]["complete"], false);
        assert!(receipt.get("read_result").is_none());
        assert!(serde_json::to_vec(&receipt).unwrap().len() < MAX_INLINE_RESULT);
    }

    #[test]
    fn long_graph_failure_preserves_host_record_id_and_warning_suffix() {
        let id = "b".repeat(64);
        let envelope = json!({"ok":false,"error":format!("Backend failed: {}; run_id={id}; run record warning: disk full", "x".repeat(20*1024))});
        let receipt = model_receipt("graph_run", &envelope);
        assert_eq!(receipt["ok"],false);
        assert_eq!(receipt["data"]["run_id"],id);
        assert_eq!(receipt["data"]["record_warning"],"disk full");
        assert_eq!(receipt["read_result"]["args"]["id"],id);
    }
}
