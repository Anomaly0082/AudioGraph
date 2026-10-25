//! Revocable local audio capabilities. No file paths are exposed to the webview.
use crate::{backend::BackendManager, tool_workspaces::ToolWorkspaces};
use serde::Serialize;
use std::{collections::HashMap, fs::{File, OpenOptions}, io::{Read, Seek, SeekFrom}, path::{Path, PathBuf}, sync::{Arc, Mutex}, time::{Duration, Instant, SystemTime}};
use tauri::{Manager, http::{Request, Response}};

const CHUNK: u64 = 2 * 1024 * 1024;
const MAX_TOKENS: usize = 16;
const IDLE: Duration = Duration::from_secs(30 * 60);
pub struct AudioStreams { entries: Mutex<HashMap<String, Stream>>, permits: Arc<tokio::sync::Semaphore> }
impl Default for AudioStreams {
    fn default() -> Self { Self {entries:Mutex::new(HashMap::new()),permits:Arc::new(tokio::sync::Semaphore::new(2))} }
}
struct Stream {
    window: String, webview: String, session: String, workspace: PathBuf,
    spaces: ToolWorkspaces, space: String, path: String, file: File,
    bytes: u64, modified: Option<SystemTime>, mime: &'static str, touched: Instant,
}
#[derive(Serialize)]
pub struct AudioOpen { token: String, bytes: u64, mime: &'static str }

fn random_token() -> Result<String,String> {
    let mut bytes = [0u8;32];
    #[cfg(windows)] {
        #[link(name="bcrypt")]
        unsafe extern "system" { fn BCryptGenRandom(algorithm: *mut std::ffi::c_void, buffer: *mut u8, length: u32, flags: u32) -> i32; }
        if unsafe { BCryptGenRandom(std::ptr::null_mut(),bytes.as_mut_ptr(),32,2) } < 0 { return Err("Cannot allocate audio capability".into()); }
    }
    #[cfg(not(windows))]
    File::open("/dev/urandom").and_then(|mut file| file.read_exact(&mut bytes)).map_err(|_| "Cannot allocate audio capability")?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
fn mime(path: &str) -> Result<&'static str,String> {
    match Path::new(path).extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
        "wav" => Ok("audio/wav"), "mp3" => Ok("audio/mpeg"), "ogg" | "oga" => Ok("audio/ogg"),
        "flac" => Ok("audio/flac"), "m4a" => Ok("audio/mp4"), "aac" => Ok("audio/aac"), "webm" => Ok("audio/webm"),
        _ => Err("Unsupported audio preview extension".into()),
    }
}
fn open_stream(window: String, webview: String, session: String, workspace: PathBuf, spaces: ToolWorkspaces, space: String, path: String) -> Result<Stream,String> {
    let mime = mime(&path)?;
    if path.is_empty() { return Err("Select an audio file".into()); }
    let target = spaces.checked_path(&space,&path,false)?;
    let mut options = OpenOptions::new(); options.read(true);
    // Windows denies writers and replacement while this capability exists, including
    // already-open writer handles. Ranges therefore cannot combine file revisions.
    #[cfg(windows)] { use std::os::windows::fs::OpenOptionsExt; options.share_mode(1); }
    let file = options.open(target).map_err(|_| "Cannot open audio; it may be in use")?;
    let metadata = file.metadata().map_err(|_| "Cannot inspect audio")?;
    let stream = Stream { window,webview,session,workspace,spaces,space,path,file,bytes:metadata.len(),modified:metadata.modified().ok(),mime,touched:Instant::now() };
    stream.check_file()?;
    Ok(stream)
}
impl Stream {
    fn belongs_to(&self, window: &str, webview: &str) -> bool {
        self.window==window && self.webview==webview
    }
    fn check_file(&self) -> Result<(),String> {
        let target = self.spaces.checked_path(&self.space,&self.path,false)?;
        let root = match self.space.as_str() { "user" => &self.spaces.user_root, "ai" => &self.spaces.ai_root, _ => return Err("Invalid space".into()) };
        let meta = self.file.metadata().map_err(|_| "Audio unavailable")?;
        let current = std::fs::metadata(target).map_err(|_| "Audio unavailable")?;
        if !meta.is_file() || meta.len()!=self.bytes || meta.modified().ok()!=self.modified
            || current.len()!=self.bytes || current.modified().ok()!=self.modified
            || !crate::run_records::opened_file_within_workspace(&self.file,root) { return Err("Audio changed or escaped workspace".into()); }
        Ok(())
    }
    fn valid(&self, backend: &BackendManager) -> bool {
        self.touched.elapsed()<IDLE && backend.workspace(&self.session).is_ok_and(|path| path==self.workspace)
    }
}
impl AudioStreams {
    pub fn cleanup(&self, backend: &BackendManager) { self.entries.lock().unwrap().retain(|_,s| s.valid(backend)); }
    pub fn close_window(&self, window: &str) { self.entries.lock().unwrap().retain(|_,s| s.window!=window); }
    fn revoke(&self, window: &str, token: &str) {
        let mut entries=self.entries.lock().unwrap();
        if entries.get(token).is_some_and(|s| s.window==window) { entries.remove(token); }
    }
    fn insert(&self, stream: Stream) -> Result<AudioOpen,String> {
        let token=random_token()?;
        let result=AudioOpen { token:token.clone(),bytes:stream.bytes,mime:stream.mime };
        let mut entries=self.entries.lock().unwrap();
        entries.retain(|_,s| s.window!=stream.window && s.touched.elapsed()<IDLE);
        if entries.len()>=MAX_TOKENS { return Err("Too many audio previews".into()); }
        entries.insert(token,stream); Ok(result)
    }
}

#[tauri::command]
pub async fn workspace_audio_open(app: tauri::AppHandle, window: tauri::Window, webview: tauri::Webview,
    backend: tauri::State<'_,Arc<BackendManager>>, session_id: String, space: String, path: String) -> Result<AudioOpen,String> {
    let manager=backend.inner().clone();
    let workspace=manager.workspace(&session_id)?;
    let data=app.path().app_data_dir().map_err(|_| "App data unavailable")?;
    let streams=app.state::<Arc<AudioStreams>>().inner().clone();
    if window.label()!=webview.label() { return Err("Audio preview requires its owning webview window".into()); }
    let window=window.label().to_owned(); let webview=webview.label().to_owned();
    let permit=streams.permits.clone().try_acquire_owned().map_err(|_| "Audio preview busy")?;
    tauri::async_runtime::spawn_blocking(move || {
        let _permit=permit;
        streams.cleanup(&manager);
        let spaces=ToolWorkspaces::new_read_only(&workspace,&data)?;
        let stream=open_stream(window,webview,session_id,workspace,spaces,space,path)?;
        if !stream.valid(&manager) { return Err("Audio session expired".into()); }
        streams.insert(stream)
    }).await.map_err(|_| "Audio open task failed".to_owned())?
}
#[tauri::command]
pub async fn workspace_audio_close(window: tauri::Window, streams: tauri::State<'_,Arc<AudioStreams>>, token: String) -> Result<(),String> {
    let streams=streams.inner().clone();
    let label=window.label().to_owned();
    tauri::async_runtime::spawn_blocking(move || streams.revoke(&label,&token)).await
        .map_err(|_| "Audio close task failed".to_owned())
}

// Single ranges are deliberately capped even when the browser requests an open
// ended range, as in Tauri's streaming example. Subsequent requests advance it.
fn range(header: Option<&str>, size: u64) -> Result<(u64,u64,bool),()> {
    if size==0 { return if header.is_none() { Ok((0,0,false)) } else { Err(()) }; }
    let Some(header)=header else { return Ok((0,size.min(CHUNK),size>CHUNK)); };
    let spec=header.strip_prefix("bytes=").ok_or(())?;
    if spec.contains(',') { return Err(()); }
    let (start,end)=spec.split_once('-').ok_or(())?;
    let number=|s: &str| -> Result<u64,()> { if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) { Err(()) } else { s.parse().map_err(|_| ()) } };
    let (start,end)=if start.is_empty() {
        let count=number(end)?; if count==0 { return Err(()); }
        (size.saturating_sub(count),size-1)
    } else {
        let start=number(start)?;
        let end=if end.is_empty() { size-1 } else { number(end)?.min(size-1) };
        if start>=size || end<start { return Err(()); } (start,end)
    };
    Ok((start,(end-start+1).min(CHUNK),true))
}
fn empty(status: u16) -> Response<Vec<u8>> { Response::builder().status(status).header("Cache-Control","no-store").body(vec![]).unwrap() }
fn serve(stream: &mut Stream, request: &Request<Vec<u8>>) -> Result<Response<Vec<u8>>,String> {
    stream.check_file()?;
    if request.method()!="GET" && request.method()!="HEAD" { return Ok(Response::builder().status(405).header("Allow","GET, HEAD").body(vec![]).unwrap()); }
    let head=request.method()=="HEAD";
    let headers=request.headers().get_all("Range");
    let mut values=headers.iter();
    let first=values.next();
    let selected=if head { range(None,stream.bytes) } else if values.next().is_some() { Err(()) } else {
        match first { Some(value)=>value.to_str().map_err(|_| ()).and_then(|value| range(Some(value),stream.bytes)), None=>range(None,stream.bytes) }
    };
    let (start,length,partial)=match selected {
        Ok(value)=>value,
        Err(_)=>return Ok(Response::builder().status(416).header("Content-Range",format!("bytes */{}",stream.bytes)).header("Accept-Ranges","bytes").header("Cache-Control","no-store").body(vec![]).unwrap()),
    };
    // HEAD reports the full representation and ignores Range, per HTTP semantics.
    let mut response=Response::builder().status(if partial && !head {206} else {200})
        .header("Content-Type",stream.mime).header("Accept-Ranges","bytes")
        .header("Cache-Control","no-store").header("X-Content-Type-Options","nosniff")
        .header("Content-Length",if head {stream.bytes} else {length});
    if partial && !head { response=response.header("Content-Range",format!("bytes {}-{}/{}",start,start+length-1,stream.bytes)); }
    let mut bytes=vec![];
    if !head && length>0 {
        bytes.resize(length as usize,0);
        stream.file.seek(SeekFrom::Start(start)).and_then(|_| stream.file.read_exact(&mut bytes)).map_err(|_| "Audio read failed")?;
    }
    stream.check_file()?; stream.touched=Instant::now();
    Ok(response.body(bytes).unwrap())
}
pub fn protocol(ctx: tauri::UriSchemeContext<'_,tauri::Wry>, request: Request<Vec<u8>>, responder: tauri::UriSchemeResponder) {
    let app=ctx.app_handle().clone();
    let label=ctx.webview_label().to_owned();
    let permits=app.state::<Arc<AudioStreams>>().permits.clone();
    let Ok(permit)=permits.try_acquire_owned() else { responder.respond(empty(503)); return; };
    tauri::async_runtime::spawn_blocking(move || {
        responder.respond(respond(&app,&label,request));
        drop(permit);
    });
}
fn respond(app: &tauri::AppHandle, label: &str, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    if let Some(origin)=request.headers().get("Origin") {
        let allowed=origin.to_str().is_ok_and(|value| matches!(value,"tauri://localhost"|"http://tauri.localhost"|"https://tauri.localhost")
            || cfg!(debug_assertions) && value=="http://localhost:1420");
        if !allowed { return empty(403); }
    }
    let registry=app.state::<Arc<AudioStreams>>();
    let backend=app.state::<Arc<BackendManager>>();
    let token=request.uri().path().strip_prefix('/').unwrap_or("");
    if token.len()!=64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) || request.uri().query().is_some() { return empty(404); }
    // A single lock bounds simultaneous file reads and allocations to one 2 MiB
    // chunk. No per-request thread or unbounded task queue is created.
    let mut entries=registry.entries.lock().unwrap();
    let Some(stream)=entries.get_mut(token) else { return empty(404); };
    let window=app.get_webview_window(label).map(|view| view.label().to_owned());
    if !window.as_deref().is_some_and(|window| stream.belongs_to(window,label)) { return empty(404); }
    if !stream.valid(&backend) { entries.remove(token); return empty(410); }
    match serve(stream,&request) { Ok(response)=>response, Err(_)=>{entries.remove(token);empty(410)} }
}

#[cfg(test)]
#[path="workspace_audio_stream_tests.rs"]
mod tests;
#[cfg(test)]
#[path="workspace_audio_stream_playback_tests.rs"]
mod playback_tests;
#[cfg(all(test,windows))]
#[path="workspace_audio_stream_webview_tests.rs"]
mod webview_tests;
