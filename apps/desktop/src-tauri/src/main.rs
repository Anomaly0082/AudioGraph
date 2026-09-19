#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::list_nodes,
            commands::run_demo_graph
        ])
        .run(tauri::generate_context!())
        .expect("failed to run AudioProcess Tauri prototype");
}

