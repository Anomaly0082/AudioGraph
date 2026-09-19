use serde::{Deserialize, Serialize};
use tauri_plugin_shell::ShellExt;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunResult {
    pub success: bool,
    pub peak: f64,
    pub gain_db: f64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortDescriptor {
    pub id: String,
    #[serde(rename = "type")]
    pub data_type: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeDescriptor {
    pub type_id: String,
    pub display_name: String,
    pub inputs: Vec<PortDescriptor>,
    pub outputs: Vec<PortDescriptor>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NodeList {
    pub nodes: Vec<NodeDescriptor>,
}

async fn run_sidecar(
    app: &tauri::AppHandle,
    args: Vec<String>,
) -> Result<String, String> {
    let command = app
        .shell()
        .sidecar("graph-demo")
        .map_err(|error| format!("无法定位 C++ graph-demo：{error}"))?
        .args(args);

    let output = command
        .output()
        .await
        .map_err(|error| format!("无法启动 C++ graph-demo：{error}"))?;

    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if message.is_empty() {
            "C++ graph-demo 执行失败".to_string()
        } else {
            message
        });
    }

    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_string())
        .map_err(|error| format!("C++ graph-demo 返回了无效 UTF-8：{error}"))
}

#[tauri::command]
pub async fn list_nodes(app: tauri::AppHandle) -> Result<NodeList, String> {
    let output = run_sidecar(&app, vec!["--list-nodes".to_string()]).await?;
    serde_json::from_str(&output).map_err(|error| format!("无法解析节点描述：{error}"))
}

#[tauri::command]
pub async fn run_demo_graph(
    app: tauri::AppHandle,
    input_path: String,
    output_path: String,
    gain_db: f64,
) -> Result<RunResult, String> {
    if input_path.trim().is_empty() || output_path.trim().is_empty() {
        return Err("输入和输出路径不能为空".to_string());
    }

    let output = run_sidecar(
        &app,
        vec![
            "--input".to_string(),
            input_path,
            "--output".to_string(),
            output_path,
            "--gain-db".to_string(),
            gain_db.to_string(),
        ],
    )
    .await?;

    serde_json::from_str(&output).map_err(|error| format!("无法解析执行结果：{error}"))
}

