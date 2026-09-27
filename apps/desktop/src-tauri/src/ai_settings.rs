//! Small local settings store. Plaintext is intentional and disclosed in the UI.
//! These commands take no file path from the frontend or from model tool arguments.
use crate::ai::{AiConfig, endpoint_url};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, atomic::{AtomicU64, Ordering}};
use tauri::Manager;

const MAX_SETTINGS_BYTES: usize = 16 * 1024;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsDocument {
    schema_version: u32,
    config: AiConfig,
}

#[derive(Serialize)]
pub struct LoadedSettings {
    path: String,
    config: Option<AiConfig>,
}

#[derive(Serialize)]
pub struct SettingsLocation { path: String }

#[derive(Default)]
pub struct SettingsStore { lock: Mutex<()> }

fn validate_config(config: &AiConfig) -> Result<(), String> {
    endpoint_url(config)?;
    if !config.api_key.trim().is_empty() {
        reqwest::header::HeaderValue::from_str(&format!("Bearer {}", config.api_key.trim()))
            .map_err(|_| "API Key包含不合法的认证头字符，未保存配置".to_owned())?;
    }
    Ok(())
}

// Only ordinary files can be read/replaced/removed. Errors never include file contents.
fn regular_file_exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(true),
        Ok(_) => Err("AI配置目标不是普通文件，未进行读写或删除".into()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(_) => Err("无法访问AI配置文件，请检查本机配置目录权限".into()),
    }
}

struct TemporarySettings(PathBuf);
impl Drop for TemporarySettings {
    fn drop(&mut self) { let _ = fs::remove_file(&self.0); }
}

impl SettingsStore {
    pub(crate) fn load(&self, path: &Path) -> Result<Option<AiConfig>, String> {
        let _guard = self.lock.lock().map_err(|_| "AI配置锁不可用")?;
        if !regular_file_exists(path)? { return Ok(None); }
        let file = File::open(path).map_err(|_| "读取AI配置失败")?;
        let mut bytes = Vec::new();
        file.take((MAX_SETTINGS_BYTES + 1) as u64).read_to_end(&mut bytes)
            .map_err(|_| "读取AI配置失败")?;
        if bytes.len() > MAX_SETTINGS_BYTES { return Err("AI配置超过16KiB，未加载".into()); }
        let value = crate::graph_files::strict_json(&bytes)
            .map_err(|_| "AI配置JSON损坏或含重复字段，原文件未修改".to_owned())?;
        let document: SettingsDocument = serde_json::from_value(value)
            .map_err(|_| "AI配置字段或版本格式无效，原文件未修改".to_owned())?;
        if document.schema_version != 1 {
            return Err("AI配置版本不受支持，原文件未修改".into());
        }
        validate_config(&document.config)?;
        Ok(Some(document.config))
    }

    pub(crate) fn save(&self, path: &Path, config: &AiConfig) -> Result<(), String> {
        let _guard = self.lock.lock().map_err(|_| "AI配置锁不可用")?;
        validate_config(config)?;
        let bytes = serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1, "config": config
        })).map_err(|_| "AI配置无法编码")?;
        if bytes.len() > MAX_SETTINGS_BYTES { return Err("AI配置超过16KiB，未保存".into()); }
        regular_file_exists(path)?;
        let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty())
            .ok_or("AI配置缺少有效父目录")?;
        fs::create_dir_all(parent).map_err(|_| "无法创建本机AI配置目录")?;
        let (temporary, mut file) = {
            let mut candidate = None;
            for _ in 0..32 {
                let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
                let name = parent.join(format!(".ai-settings-{}-{id}.tmp", std::process::id()));
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)] {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                match options.open(&name) {
                    Ok(file) => { candidate = Some((TemporarySettings(name), file)); break; }
                    Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                    Err(_) => return Err("无法创建临时AI配置，旧配置未修改".into()),
                }
            }
            candidate.ok_or("无法创建唯一临时AI配置，旧配置未修改")?
        };
        // Drop the handle before rename on Windows. The guard removes only our own temp file.
        let written = file.write_all(&bytes).and_then(|_| file.sync_all());
        drop(file);
        written.map_err(|_| "写入AI配置失败，旧配置未修改")?;
        regular_file_exists(path)?;
        // Same-directory atomic replacement: never delete the old config before a successful write.
        fs::rename(&temporary.0, path).map_err(|_| "替换AI配置失败，旧配置未修改")?;
        Ok(())
    }

    pub(crate) fn clear(&self, path: &Path) -> Result<(), String> {
        let _guard = self.lock.lock().map_err(|_| "AI配置锁不可用")?;
        if !regular_file_exists(path)? { return Ok(()); }
        fs::remove_file(path).map_err(|_| "删除本机AI配置失败".to_owned())
    }
}

fn settings_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path().app_config_dir().map(|directory| directory.join("ai-settings.json"))
        .map_err(|_| "无法确定本机用户配置目录".into())
}

#[tauri::command]
pub async fn ai_load_settings(app: tauri::AppHandle, store: tauri::State<'_, Arc<SettingsStore>>)
    -> Result<LoadedSettings, String> {
    let path = settings_path(&app)?;
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        store.load(&path).map(|config| LoadedSettings { path: path.to_string_lossy().into_owned(), config })
    }).await.map_err(|_| "AI配置加载任务失败".to_owned())?
}

#[tauri::command]
pub async fn ai_save_settings(app: tauri::AppHandle, store: tauri::State<'_, Arc<SettingsStore>>, config: AiConfig)
    -> Result<SettingsLocation, String> {
    let path = settings_path(&app)?;
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        store.save(&path, &config).map(|()| SettingsLocation { path: path.to_string_lossy().into_owned() })
    }).await.map_err(|_| "AI配置保存任务失败".to_owned())?
}

#[tauri::command]
pub async fn ai_clear_settings(app: tauri::AppHandle, store: tauri::State<'_, Arc<SettingsStore>>)
    -> Result<SettingsLocation, String> {
    let path = settings_path(&app)?;
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        store.clear(&path).map(|()| SettingsLocation { path: path.to_string_lossy().into_owned() })
    }).await.map_err(|_| "AI配置清除任务失败".to_owned())?
}

#[cfg(test)]
#[path = "ai_settings_tests.rs"]
mod tests;
