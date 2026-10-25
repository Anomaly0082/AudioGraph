//! Explicit opt-in native WebView2 smoke test. The sole window stays invisible,
//! but a crashing WebView2 child can still show a Windows error dialog. Never run
//! this while the user requires uninterrupted desktop use; use the headless bridge.
use super::*;
use std::{borrow::Cow, io::Write, sync::atomic::{AtomicBool,AtomicUsize,AtomicU64,Ordering}};
use tauri::utils::assets::{AssetKey,AssetsIter,CspHash};

struct BlankAssets;
const HTML: &[u8]=b"<!doctype html><html><head><meta charset='utf-8'></head><body></body></html>";
impl tauri::Assets<tauri::Wry> for BlankAssets {
    fn get(&self,key: &AssetKey) -> Option<Cow<'_,[u8]>> {
        matches!(key.as_ref(),"/index.html"|"index.html"|"/").then_some(Cow::Borrowed(HTML))
    }
    fn iter(&self)->Box<AssetsIter<'_>> { Box::new(std::iter::once((Cow::Borrowed("/index.html"),Cow::Borrowed(HTML)))) }
    fn csp_hashes(&self,_: &AssetKey)->Box<dyn Iterator<Item=CspHash<'_>>+'_> { Box::new(std::iter::empty()) }
}
struct Fixture(PathBuf);
impl Drop for Fixture { fn drop(&mut self) { let _=std::fs::remove_dir_all(&self.0); } }

#[test]
#[ignore="Native WebView2 may show system crash dialogs; requires explicit permission for desktop interruption"]
fn hidden_webview2_protocol_playback() {
    assert_eq!(std::env::var("AUDIOGRAPH_ALLOW_NATIVE_WEBVIEW_DIALOGS").as_deref(),Ok("1"),
        "Native runtime can show system dialogs even with a hidden window; obtain explicit user permission first");
    assert_eq!(std::env::var("AUDIOGRAPH_HIDDEN_WEBVIEW_TEST").as_deref(),Ok("1"));
    let fixture=Fixture(std::env::temp_dir().join(format!("audio-hidden-webview-{}",random_token().unwrap())));
    let user=fixture.0.join("user"); let data=fixture.0.join("data");
    std::fs::create_dir_all(&user).unwrap();
    let spaces=ToolWorkspaces::new_read_only(&user,&data).unwrap();
    let frames=48000u32*240; let data_bytes=frames*4;
    let mut header=Vec::new();
    header.extend(b"RIFF"); header.extend((36+data_bytes).to_le_bytes()); header.extend(b"WAVEfmt ");
    header.extend(16u32.to_le_bytes()); header.extend(1u16.to_le_bytes()); header.extend(2u16.to_le_bytes());
    header.extend(48000u32.to_le_bytes()); header.extend(192000u32.to_le_bytes());
    header.extend(4u16.to_le_bytes()); header.extend(16u16.to_le_bytes()); header.extend(b"data"); header.extend(data_bytes.to_le_bytes());
    let mut file=File::create(user.join("large.wav")).unwrap(); file.write_all(&header).unwrap();
    file.set_len(44+u64::from(data_bytes)).unwrap(); drop(file);
    let project=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let executable=[project.join("build/Debug/control-cli.exe"),project.join("build/Release/control-cli.exe")].into_iter()
        .find(|path| path.is_file()).expect("Build control-cli first");
    let session="hidden-webview-test";
    let connection=crate::backend::Connection::spawn(&executable,spaces.user_root.clone(),session.into(),false,false,Arc::new(|_| {})).unwrap();
    let backend=Arc::new(BackendManager::with_test_connection(connection));
    let streams=Arc::new(AudioStreams::default());
    let audio=streams.insert(open_stream("preview-test".into(),"preview-test".into(),session.into(),spaces.user_root.clone(),spaces,"user".into(),"large.wav".into()).unwrap()).unwrap();
    let result=Arc::new(Mutex::new(None::<serde_json::Value>));
    let done=Arc::new(AtomicBool::new(false));
    let requests=Arc::new(AtomicUsize::new(0)); let max_offset=Arc::new(AtomicU64::new(0));
    let script=format!(r#"
document.addEventListener('DOMContentLoaded',async()=>{{
 const a=document.createElement('audio'); a.muted=true; a.volume=0; a.preload='auto'; document.body.append(a);
 const once=(name)=>new Promise((resolve,reject)=>{{a.addEventListener(name,resolve,{{once:true}});a.addEventListener('error',()=>reject(new Error('media '+(a.error?.code))),{{once:true}});}});
 const finish=(result)=>{{a.pause();a.removeAttribute('src');a.load();location.href='http://tauri.localhost/finished?result='+encodeURIComponent(JSON.stringify(result));}};
 try {{
  const metadata=once('loadedmetadata');a.src='http://audiopreview.localhost/{}';a.load();await metadata;
  if(Math.abs(a.duration-240)>0.1)throw new Error('duration '+a.duration);
  await a.play();await new Promise(r=>setTimeout(r,250));a.pause();
  if(a.currentTime<=0)throw new Error('play did not advance');
  const seeked=once('seeked');a.currentTime=230;await seeked;
  await a.play();await new Promise(r=>setTimeout(r,250));a.pause();
  if(a.currentTime<230)throw new Error('seek failed '+a.currentTime);
  finish({{ok:true,duration:a.duration,currentTime:a.currentTime,muted:a.muted}});
 }}catch(e){{finish({{ok:false,error:String(e)}});}}
}},{{once:true}});
"#,audio.token);
    let mut context=tauri::generate_context!();
    context.config_mut().app.windows.clear(); context.config_mut().build.dev_url=None;
    context.config_mut().app.security.csp=None;
    context.set_assets(Box::new(BlankAssets));
    let protocol_requests=requests.clone(); let protocol_offset=max_offset.clone();
    let setup_result=result.clone(); let setup_done=done.clone(); let profile=fixture.0.join("webview-profile");
    let app=tauri::Builder::default().any_thread().manage(backend.clone()).manage(streams.clone())
        .register_asynchronous_uri_scheme_protocol("audiopreview",move |ctx,request,responder| {
            protocol_requests.fetch_add(1,Ordering::Relaxed);
            if let Some(offset)=request.headers().get("Range").and_then(|h| h.to_str().ok()).and_then(|s| s.strip_prefix("bytes=")).and_then(|s| s.split('-').next()).and_then(|s| s.parse::<u64>().ok()) {
                protocol_offset.fetch_max(offset,Ordering::Relaxed);
            }
            protocol(ctx,request,responder);
        })
        .setup(move |app| {
            println!("HIDDEN_WEBVIEW_SETUP");
            let watchdog_app=app.handle().clone(); let watchdog_done=setup_done.clone();
            std::thread::spawn(move || {
                for _ in 0..450 { if watchdog_done.load(Ordering::Acquire) {return;} std::thread::sleep(Duration::from_millis(100)); }
                watchdog_app.exit(1);
            });
            let handle=app.handle().clone(); let navigation_result=setup_result.clone(); let navigation_done=setup_done.clone();
            tauri::WebviewWindowBuilder::new(app,"preview-test",tauri::WebviewUrl::App("index.html".into()))
                .visible(false).focused(false).skip_taskbar(true).data_directory(profile.clone())
                .initialization_script(script.clone())
                .on_navigation(move |url| {
                    println!("HIDDEN_WEBVIEW_NAVIGATION {}",url.path());
                    if url.host_str()==Some("tauri.localhost") && url.path()=="/finished" {
                        let value=url.query_pairs().find(|(key,_)| key=="result").and_then(|(_,value)| serde_json::from_str(&value).ok());
                        *navigation_result.lock().unwrap()=value; navigation_done.store(true,Ordering::Release); handle.exit(0); false
                    } else { url.host_str()==Some("tauri.localhost") }
                }).build()?;
            println!("HIDDEN_WEBVIEW_CREATED");
            Ok(())
        }).build(context).expect("Cannot build hidden WebView2 test app");
    let exit=app.run_return(|_,_| {});
    done.store(true,Ordering::Release);
    streams.close_window("preview-test"); let shutdown=backend.shutdown();
    let captured=result.lock().unwrap().clone();
    println!("HIDDEN_WEBVIEW_RESULT {}",serde_json::json!({"result":captured,"requests":requests.load(Ordering::Relaxed),"max_offset":max_offset.load(Ordering::Relaxed),"exit":exit}));
    assert!(shutdown.is_ok(),"Backend shutdown failed"); assert_eq!(exit,0,"Hidden WebView watchdog expired");
    let captured=captured.expect("WebView did not return a result"); assert_eq!(captured["ok"],true,"{captured}");
    assert_eq!(captured["muted"],true); assert!(requests.load(Ordering::Relaxed)>=2);
    assert!(max_offset.load(Ordering::Relaxed)>32*1024*1024,"Native seek must request bytes beyond old preview limit");
}
