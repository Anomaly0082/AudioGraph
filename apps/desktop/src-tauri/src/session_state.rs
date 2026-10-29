//! Local editor recovery and remembered audio permission choices; never executes tasks.
//! Writes are serialized in this process. Cross-process writes use last-writer-wins.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs::{self, File}, io::{Read, Write}, path::{Component, Path, PathBuf}, sync::{Arc, Mutex}};
use tauri::Manager;

const MAX_BYTES: usize = 20 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    schema_version: u32, #[serde(deserialize_with="required_option")] workspace: Option<String>, page: Page,
    #[serde(rename="editorKind")] editor_kind: EditorKind,
    graph: Graph, workflow: Workflow,
    #[serde(default)] permissions: Permissions,
}
// Legacy recovery files omit this object. Present objects must contain both flags.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all="camelCase")]
struct Permissions { allow_devices: bool, allow_monitor: bool }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all="lowercase")]
enum Page { Workbench, Editor, Files, Tasks, Settings }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all="lowercase")]
enum EditorKind { Graph, Workflow }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all="lowercase")]
enum Mode { Offline, Streaming, Realtime }
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all="lowercase")]
enum Space { User, Ai }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all="camelCase")]
struct Graph { text: String, mode: Mode, block_frames: String, duration: String, canvas_view: Canvas, #[serde(deserialize_with="required_option")] source: Option<Source> }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all="camelCase")]
struct Workflow { text: String, source_space: Space, path: String, #[serde(deserialize_with="required_option")] source: Option<Source> }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all="camelCase")]
struct Canvas { positions: BTreeMap<String, Point>, selected_id: String, zoom: f64, pan: Point }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Point { x: f64, y: f64 }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source { workspace: String, path: String, revision: String, content: String, space: Space }

fn required_option<'de,D: serde::Deserializer<'de>,T: Deserialize<'de>>(deserializer:D)->Result<Option<T>,D::Error> { Option::<T>::deserialize(deserializer) }

fn absolute(value: &str) -> bool {
    !value.is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control)
        && Path::new(value).is_absolute()
        && !value.split(['/', '\\']).any(|p| p == "." || p == "..")
        && !Path::new(value).components().any(|p| matches!(p, Component::CurDir | Component::ParentDir))
}
fn identity(value: &str) -> String {
    #[cfg(windows)] { windows_identity(value) }
    #[cfg(not(windows))] { value.to_owned() }
}
fn windows_identity(value: &str) -> String {
    let slashes=value.replace('\\',"/");
    let lowered=slashes.to_lowercase();
    let plain=if lowered.starts_with("//?/unc/") { format!("//{}",&lowered[8..]) }
        else if lowered.starts_with("//?/") { lowered[4..].to_owned() } else { lowered };
    plain.trim_end_matches('/').to_owned()
}
fn inside(workspace: &str, path: &str)->bool {
    let root=identity(workspace); let file=identity(path);
    file.strip_prefix(&format!("{root}/")).is_some_and(|remaining|!remaining.is_empty())
}
fn relative(value: &str) -> bool {
    if value.is_empty() || value.len()>4096 || value.starts_with('/') || value.contains(['\\', ':', '\0']) { return false; }
    value.split('/').all(|p| {
        let stem = p.split('.').next().unwrap_or("").to_ascii_uppercase();
        !p.is_empty() && p != "." && p != ".." && !p.ends_with(['.', ' '])
            && !p.chars().any(|c| c.is_control() || "<>\"|?*".contains(c))
            && !matches!(stem.as_str(), "CON"|"PRN"|"AUX"|"NUL"|"COM1"|"COM2"|"COM3"|"COM4"|"COM5"|"COM6"|"COM7"|"COM8"|"COM9"|"LPT1"|"LPT2"|"LPT3"|"LPT4"|"LPT5"|"LPT6"|"LPT7"|"LPT8"|"LPT9")
    })
}
fn point(p: &Point) -> bool { p.x.is_finite() && p.y.is_finite() && p.x.abs()<=1e7 && p.y.abs()<=1e7 }
fn source(s: &Source, workspace: &Option<String>, graph: bool, limit: usize) -> bool {
    workspace.as_ref().is_some_and(|w| identity(w)==identity(&s.workspace))
        && s.content.len()<=limit && s.revision.len()==64
        && s.revision.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && if graph {
            s.space==Space::User && absolute(&s.path)
                && inside(&s.workspace,&s.path)
        } else { relative(&s.path) }
}
fn validate(doc: &Document) -> Result<(), String> {
    let g=&doc.graph; let w=&doc.workflow; let c=&g.canvas_view;
    if doc.schema_version!=1 || doc.workspace.as_ref().is_some_and(|p| !absolute(p))
        || (doc.permissions.allow_monitor && !doc.permissions.allow_devices)
        || g.text.len()>4*1024*1024 || w.text.len()>64*1024 || g.block_frames.len()>32 || g.duration.len()>32
        || w.path.len()>4096
        || c.positions.len()>4096 || c.selected_id.len()>1024 || !point(&c.pan)
        || !c.zoom.is_finite() || !(0.25..=4.0).contains(&c.zoom)
        || c.positions.iter().any(|(id,p)| id.len()>1024 || !point(p))
        || g.source.as_ref().is_some_and(|s| !source(s,&doc.workspace,true,4*1024*1024))
        || w.source.as_ref().is_some_and(|s| !source(s,&doc.workspace,false,64*1024))
    { return Err("编辑器恢复记录格式、路径或大小无效".into()); }
    Ok(())
}
fn regular(path: &Path) -> Result<bool,String> {
    crate::run_records::check_chain(path)?;
    match crate::run_records::safe_metadata(path)? {
        None=>Ok(false), Some(m) if m.is_file() && !crate::run_records::is_hardlinked(path,&m)=>Ok(true),
        _=>Err("编辑器恢复记录不是安全的普通文件，原文件未修改".into())
    }
}
fn read(path: &Path) -> Result<Option<Document>,String> {
    if !regular(path)? { return Ok(None); }
    let file=File::open(path).map_err(|_| "无法读取编辑器恢复记录")?;
    if !crate::run_records::opened_file_within_workspace(&file,path.parent().ok_or("配置路径无效")?) || !regular(path)? { return Err("恢复记录读取期间发生变化".into()); }
    let mut bytes=Vec::new(); file.take((MAX_BYTES+1) as u64).read_to_end(&mut bytes).map_err(|_| "无法读取编辑器恢复记录")?;
    if bytes.len()>MAX_BYTES { return Err("编辑器恢复记录超过20MiB，原文件未修改".into()); }
    let value=crate::graph_files::strict_json(&bytes).map_err(|_| "编辑器恢复记录损坏，原文件未修改")?;
    let doc: Document=serde_json::from_value(value).map_err(|_| "编辑器恢复记录格式无效，原文件未修改")?;
    validate(&doc)?; Ok(Some(doc))
}
fn write(path: &Path, bytes: &[u8]) -> Result<(),String> {
    if bytes.len()>MAX_BYTES { return Err("编辑器恢复记录超过20MiB，未保存且未截断".into()); }
    let existed=regular(path)?; let parent=path.parent().ok_or("配置路径无效")?;
    crate::run_records::check_chain(parent)?; fs::create_dir_all(parent).map_err(|_| "无法创建配置目录")?;
    crate::run_records::check_chain(parent)?;
    let (temp,mut file)=crate::run_records::temp_file(parent)?;
    let result=(|| {
        file.write_all(bytes).and_then(|_| file.sync_all()).map_err(|_| "无法保存编辑器恢复记录")?; drop(file);
        if existed!=regular(path)? { return Err("恢复记录保存期间发生变化".into()); }
        if existed { crate::run_records::replace_file(&temp,path) } else { crate::run_records::create_file(&temp,path) }
    })();
    let _=fs::remove_file(temp); result
}
#[derive(Default)]
pub struct SessionStateStore { ready: Mutex<bool> }
impl SessionStateStore {
    fn load(&self,path: &Path)->Result<Loaded,String> {
        let mut ready=self.ready.lock().map_err(|_| "恢复记录锁不可用")?;
        *ready=false; let state=read(path)?; *ready=true; Ok(Loaded { state })
    }
    fn save(&self,path: &Path,doc: Document)->Result<(),String> {
        let ready=self.ready.lock().map_err(|_| "恢复记录锁不可用")?;
        if !*ready { return Err("恢复记录尚未成功读取；损坏记录须显式清除后才能保存".into()); }
        validate(&doc)?; let bytes=serde_json::to_vec(&doc).map_err(|_| "无法编码恢复记录")?; write(path,&bytes)
    }
    fn clear(&self,path: &Path)->Result<(),String> {
        let mut ready=self.ready.lock().map_err(|_| "恢复记录锁不可用")?;
        // Explicit clear replaces only this safe local metadata file.
        if regular(path)? { fs::remove_file(path).map_err(|_| "无法清除编辑器恢复记录")?; }
        *ready=true; Ok(())
    }
}
#[derive(Serialize)] pub struct Loaded { state: Option<Document> }
fn storage(app: &tauri::AppHandle)->Result<PathBuf,String> { app.path().app_config_dir().map(|p|p.join("session-state.json")).map_err(|_|"无法确定配置目录".into()) }
#[tauri::command]
pub async fn session_state_load(app: tauri::AppHandle,store: tauri::State<'_,Arc<SessionStateStore>>)->Result<Loaded,String> {
    let path=storage(&app)?; let store=store.inner().clone(); tauri::async_runtime::spawn_blocking(move||store.load(&path)).await.map_err(|_|"恢复记录读取任务失败")?
}
#[tauri::command]
pub async fn session_state_save(app: tauri::AppHandle,store: tauri::State<'_,Arc<SessionStateStore>>,state: Document)->Result<(),String> {
    let path=storage(&app)?; let store=store.inner().clone(); tauri::async_runtime::spawn_blocking(move||store.save(&path,state)).await.map_err(|_|"恢复记录保存任务失败")?
}
#[tauri::command]
pub async fn session_state_clear(app: tauri::AppHandle,store: tauri::State<'_,Arc<SessionStateStore>>)->Result<(),String> {
    let path=storage(&app)?; let store=store.inner().clone(); tauri::async_runtime::spawn_blocking(move||store.clear(&path)).await.map_err(|_|"恢复记录清除任务失败")?
}

#[derive(Default)] pub struct CloseCoordinator { state: Mutex<CloseState> }
#[derive(Default)] struct CloseState { sequence: u64, pending: Option<(String,bool,bool)>, restart: bool }
impl CloseCoordinator {
    pub fn request(&self)->Option<String> { self.request_action(false) }
    pub fn request_restart(&self)->Option<String> { self.request_action(true) }
    fn request_action(&self,restart:bool)->Option<String> { let mut s=self.state.lock().ok()?; if s.pending.is_some(){return None;} s.sequence+=1; let id=format!("{}-{}",std::process::id(),s.sequence); s.pending=Some((id.clone(),false,false)); s.restart=restart; Some(id) }
    pub fn is_restart(&self,id:&str)->bool { self.state.lock().is_ok_and(|s|s.restart && s.pending.as_ref().is_some_and(|(current,_,_)|current==id)) }
    pub fn claimed_restart(&self)->bool { self.state.lock().is_ok_and(|s|s.restart && s.pending.as_ref().is_some_and(|(_,claimed,_)|*claimed)) }
    pub fn claim(&self,id:&str)->bool { let Ok(mut s)=self.state.lock() else{return false}; if let Some((current,claimed,_))=&mut s.pending { if current==id && !*claimed { *claimed=true; return true; } } false }
    pub fn waiting(&self,id:&str)->bool { self.state.lock().is_ok_and(|s|s.pending.as_ref().is_some_and(|(current,claimed,confirmation)|current==id && !claimed && !confirmation)) }
    /// Frontend and timeout fallback compete for one confirmation dialog slot.
    pub fn begin_confirmation(&self,id:&str)->bool { let Ok(mut s)=self.state.lock() else{return false}; if let Some((current,claimed,confirmation))=&mut s.pending { if current==id && !*claimed && !*confirmation { *confirmation=true; return true; } } false }
    pub fn cancel(&self,id:&str)->bool { let Ok(mut s)=self.state.lock() else{return false}; if s.pending.as_ref().is_some_and(|(current,claimed,_)|current==id && !claimed) {s.pending=None;true}else{false} }
    pub fn reset(&self)->Option<String> { self.state.lock().ok()?.pending.take().map(|(id,_,_)|id) }
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn restart_intent_is_bound_to_one_acknowledged_close() {
        let c=CloseCoordinator::default();
        let restart=c.request_restart().unwrap();
        assert!(c.request().is_none());
        assert!(c.is_restart(&restart));
        assert!(!c.claimed_restart());
        assert!(!c.is_restart("stale"));
        assert!(c.cancel(&restart));
        assert!(!c.claim(&restart));
        assert!(!c.claimed_restart());
        let close=c.request().unwrap();
        assert!(c.request_restart().is_none());
        assert!(c.claim(&close));
        assert!(!c.claimed_restart());
        c.reset();
        let retry=c.request_restart().unwrap();
        assert!(!c.claim(&restart));
        assert!(c.claim(&retry));
        assert!(c.claimed_restart());
        assert!(!c.cancel(&retry));
        c.reset();
        assert!(!c.claimed_restart());
    }
    #[test] fn permission_choices_roundtrip_and_legacy_defaults_are_safe() {
        let legacy=doc();
        assert!(!legacy.permissions.allow_devices && !legacy.permissions.allow_monitor);
        for (devices,monitor) in [(false,false),(true,false),(true,true)] {
            let mut d=doc(); d.permissions=Permissions{allow_devices:devices,allow_monitor:monitor};
            let loaded:Document=serde_json::from_slice(&serde_json::to_vec(&d).unwrap()).unwrap();
            assert!(validate(&loaded).is_ok());
            assert_eq!(loaded.permissions.allow_devices,devices);
            assert_eq!(loaded.permissions.allow_monitor,monitor);
        }
        let mut invalid=doc();invalid.permissions.allow_monitor=true;assert!(validate(&invalid).is_err());
        for permissions in [serde_json::json!(null),serde_json::json!({}),serde_json::json!({"allowDevices":"true","allowMonitor":false}),serde_json::json!({"allowDevices":true,"allowMonitor":false,"execute":true})] {
            let mut value=serde_json::to_value(doc()).unwrap();value["permissions"]=permissions;
            assert!(serde_json::from_value::<Document>(value).is_err());
        }
    }
    fn doc()->Document { serde_json::from_value(serde_json::json!({"schema_version":1,"workspace":null,"page":"editor","editorKind":"graph","graph":{"text":"{ invalid draft","mode":"offline","blockFrames":"bad","duration":"","canvasView":{"positions":{},"selectedId":"","zoom":1,"pan":{"x":0,"y":0}},"source":null},"workflow":{"text":"invalid","sourceSpace":"user","path":"","source":null}})).unwrap() }
    #[test] fn validation_preserves_invalid_drafts_but_rejects_authority_and_bounds(){let mut d=doc(); assert!(validate(&d).is_ok()); d.graph.canvas_view.zoom=4.1; assert!(validate(&d).is_err()); d=doc();d.workflow.path="../unfinished input".into();assert!(validate(&d).is_ok());d.workflow.path="x".repeat(4097);assert!(validate(&d).is_err());d=doc();d.graph.text="x".repeat(4*1024*1024+1);assert!(validate(&d).is_err());}
    #[test] fn windows_drive_unc_slash_and_trailing_separator_identity(){
        assert_eq!(windows_identity(r"\\?\C:\Audio\"),windows_identity("c:/audio"));
        assert_eq!(windows_identity(r"\\?\UNC\Server\Share\Audio\"),windows_identity("//server/share/audio"));
        assert_eq!(windows_identity(r"C:\"),windows_identity("c:"));
        #[cfg(windows)] {
            assert!(inside(r"\\?\C:\Audio\", "c:/audio/sub/file.json"));
            assert!(inside("//server/share/audio",r"\\?\UNC\SERVER\Share\Audio\file.json"));
            assert!(!inside("c:/audio", "c:/audio-other/file.json"));
            assert!(!inside("c:/audio", "c:/audio"));
            for (workspace,owner,path) in [(r"\\?\C:\Audio\","c:/audio","C:/AUDIO/draft.json"),(r"\\?\UNC\Server\Share\Audio","//server/share/audio",r"\\SERVER\Share\Audio\draft.json")] {
                let mut d=doc();d.workspace=Some(workspace.into());
                d.graph.source=Some(Source{workspace:owner.into(),path:path.into(),revision:"a".repeat(64),content:"draft".into(),space:Space::User});
                assert!(validate(&d).is_ok());
            }
        }
    }
    #[test] fn close_ids_deduplicate_and_reject_stale_ack(){let c=CloseCoordinator::default();let first=c.request().unwrap();assert!(c.request().is_none());assert!(c.cancel(&first));let second=c.request().unwrap();assert_ne!(first,second);assert!(!c.claim(&first));assert!(c.claim(&second));assert!(!c.claim(&second));assert!(!c.cancel(&second));c.reset();assert!(c.request().is_some());}
    #[test] fn confirmation_slot_is_mutually_exclusive_and_keeps_finish_cancel_available(){
        let c=CloseCoordinator::default();let first=c.request().unwrap();assert!(!c.begin_confirmation("stale"));assert!(c.waiting(&first));assert!(c.begin_confirmation(&first));assert!(!c.waiting(&first));assert!(!c.begin_confirmation(&first));assert!(c.cancel(&first));
        let second=c.request().unwrap();assert!(!c.begin_confirmation(&first));assert!(c.begin_confirmation(&second));assert!(c.claim(&second));assert!(!c.begin_confirmation(&second));assert!(!c.cancel(&second));
        c.reset();let third=c.request().unwrap();assert!(c.waiting(&third));assert!(c.claim(&third));assert!(!c.begin_confirmation(&third));
    }
    #[test] fn corrupt_file_blocks_save_until_explicit_clear(){let root=std::env::temp_dir().join(format!("session-recovery-test-{}",std::process::id()));fs::create_dir_all(&root).unwrap();let path=root.join("session-state.json");fs::write(&path,b"{broken").unwrap();let store=SessionStateStore::default();assert!(store.load(&path).is_err());assert!(store.save(&path,doc()).is_err());assert_eq!(fs::read(&path).unwrap(),b"{broken");store.clear(&path).unwrap();assert!(store.load(&path).unwrap().state.is_none());store.save(&path,doc()).unwrap();assert_eq!(store.load(&path).unwrap().state.unwrap().graph.text,"{ invalid draft");fs::remove_file(path).unwrap();fs::remove_dir(root).unwrap();}
    #[test] fn strict_parse_rejects_duplicate_and_unknown_fields(){assert!(crate::graph_files::strict_json(br#"{"schema_version":1,"schema_version":1}"#).is_err());let mut value=serde_json::to_value(doc()).unwrap();value["permissions"]=serde_json::json!({});assert!(serde_json::from_value::<Document>(value).is_err());}
    #[test] fn sources_are_lexical_and_never_open_workspace_files(){
        let workspace=std::env::temp_dir().join("nonexistent-recovery-workspace").to_string_lossy().into_owned();
        let mut d=doc(); d.workspace=Some(workspace.clone());
        d.graph.source=Some(Source{workspace:workspace.clone(),path:Path::new(&workspace).join("draft.json").to_string_lossy().into_owned(),revision:"a".repeat(64),content:"invalid graph source".into(),space:Space::User});
        assert!(validate(&d).is_ok());
        d.graph.source.as_mut().unwrap().space=Space::Ai; assert!(validate(&d).is_err());
        d.graph.source.as_mut().unwrap().space=Space::User;
        d.graph.source.as_mut().unwrap().path=Path::new(&workspace).join("../outside.json").to_string_lossy().into_owned(); assert!(validate(&d).is_err());
        d.graph.source=None; d.workflow.source=Some(Source{workspace,path:"flows/draft.json".into(),revision:"b".repeat(64),content:"invalid workflow".into(),space:Space::Ai}); assert!(validate(&d).is_ok());
        d.workflow.source.as_mut().unwrap().revision="B".repeat(64);assert!(validate(&d).is_err());
    }
    #[test] fn serialized_limit_and_hardlinks_preserve_original(){
        let root=std::env::temp_dir().join(format!("session-recovery-links-test-{}",std::process::id()));fs::create_dir_all(&root).unwrap();
        let path=root.join("session-state.json");let store=SessionStateStore::default();store.load(&path).unwrap();store.save(&path,doc()).unwrap();
        let original=fs::read(&path).unwrap();
        assert!(write(&path,&vec![b'x';MAX_BYTES+1]).is_err());assert_eq!(fs::read(&path).unwrap(),original);
        let link=root.join("linked.json");fs::hard_link(&path,&link).unwrap();assert!(store.load(&path).is_err());assert!(store.clear(&path).is_err());assert_eq!(fs::read(&path).unwrap(),original);
        fs::remove_file(link).unwrap();fs::remove_file(path).unwrap();fs::remove_dir(root).unwrap();
    }
}
