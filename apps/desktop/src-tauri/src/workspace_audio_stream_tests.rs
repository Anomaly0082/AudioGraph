use super::*;
use std::sync::atomic::{AtomicU64,Ordering};
static NEXT: AtomicU64=AtomicU64::new(0);
fn fixture() -> (PathBuf,ToolWorkspaces) {
    let directory=std::env::temp_dir().join(format!("audio-stream-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
    std::fs::create_dir_all(directory.join("user")).unwrap();
    let spaces=ToolWorkspaces::new_read_only(&directory.join("user"),&directory.join("data")).unwrap();
    (directory,spaces)
}
fn stream(spaces: &ToolWorkspaces) -> Stream {
    open_stream("main".into(),"main".into(),"session".into(),spaces.user_root.clone(),spaces.clone(),"user".into(),"large.wav".into()).unwrap()
}
#[test]
fn range_boundaries_are_bounded_and_strict() {
    assert_eq!(range(Some("bytes=0-0"),10),Ok((0,1,true)));
    assert_eq!(range(Some("bytes=8-99"),10),Ok((8,2,true)));
    assert_eq!(range(Some("bytes=-3"),10),Ok((7,3,true)));
    assert_eq!(range(Some("bytes=-99"),10),Ok((0,10,true)));
    assert_eq!(range(Some("bytes=0-"),CHUNK*5),Ok((0,CHUNK,true)));
    assert_eq!(range(None,CHUNK*5),Ok((0,CHUNK,true)));
    assert_eq!(range(None,0),Ok((0,0,false)));
    for bad in ["bytes=10-","bytes=8-7","bytes=-0","bytes=-","bytes=1-2,4-5","bytes=+1-2","bytes=18446744073709551616-","items=0-1","bytes= 0-1"] {
        assert!(range(Some(bad),10).is_err(),"{bad}");
    }
    assert!(range(Some("bytes=0-0"),0).is_err());
}
#[test]
fn audio_extensions_are_explicit_not_a_general_asset_reader() {
    for name in ["test.WAV","test.mp3","test.ogg","test.oga","test.flac","test.m4a","test.aac","test.webm"] {
        assert!(mime(name).unwrap().starts_with("audio/"));
    }
    for name in ["test.html","test.svg","test.js","test.txt","test.exe","test.wav.html"] {
        assert!(mime(name).is_err());
    }
}
#[test]
fn large_sparse_file_ranges_head_and_revocation() {
    let (directory,spaces)=fixture();
    let size=40*1024*1024+44;
    let mut file=OpenOptions::new().create_new(true).write(true).open(spaces.user_root.join("large.wav")).unwrap();
    file.set_len(size).unwrap();
    use std::io::Write;
    file.write_all(b"RIFF").unwrap(); file.seek(SeekFrom::End(-4)).unwrap(); file.write_all(b"tail").unwrap(); drop(file);
    let mut audio=stream(&spaces);
    assert!(audio.belongs_to("main","main"));
    assert!(!audio.belongs_to("other","main"));
    assert!(!audio.belongs_to("main","other"));
    let get=|method: &str,range: Option<&str>| { let mut req=Request::builder().method(method); if let Some(value)=range {req=req.header("Range",value);} req.body(vec![]).unwrap() };
    let response=serve(&mut audio,&get("GET",Some("bytes=0-3"))).unwrap();
    assert_eq!(response.status(),206); assert_eq!(response.body(),b"RIFF");
    assert_eq!(response.headers()["Content-Range"],format!("bytes 0-3/{size}"));
    assert_eq!(serve(&mut audio,&get("GET",Some("bytes=-4"))).unwrap().body(),b"tail");
    let response=serve(&mut audio,&get("GET",None)).unwrap(); assert_eq!(response.status(),206); assert_eq!(response.body().len(),CHUNK as usize);
    let response=serve(&mut audio,&get("HEAD",Some("bytes=999999999-"))).unwrap();
    assert_eq!(response.status(),200); assert!(response.body().is_empty()); assert_eq!(response.headers()["Content-Length"],size.to_string());
    let response=serve(&mut audio,&get("GET",Some("bytes=999999999-"))).unwrap(); assert_eq!(response.status(),416); assert_eq!(response.headers()["Content-Range"],format!("bytes */{size}"));
    assert_eq!(serve(&mut audio,&get("POST",None)).unwrap().status(),405);
    let registry=AudioStreams::default(); let opened=registry.insert(audio).unwrap(); assert_eq!(opened.token.len(),64);
    registry.revoke("other",&opened.token); assert_eq!(registry.entries.lock().unwrap().len(),1);
    registry.revoke("main",&opened.token); registry.revoke("main",&opened.token); assert!(registry.entries.lock().unwrap().is_empty());
    std::fs::remove_dir_all(directory).unwrap();
}
#[test]
fn replacement_invalidates_token_and_invalid_session_releases_handles() {
    let (directory,spaces)=fixture(); std::fs::write(spaces.user_root.join("large.wav"),b"RIFF").unwrap();
    let registry=AudioStreams::default(); let first=registry.insert(stream(&spaces)).unwrap(); let second=registry.insert(stream(&spaces)).unwrap();
    assert_ne!(first.token,second.token); assert!(!registry.entries.lock().unwrap().contains_key(&first.token));
    registry.cleanup(&BackendManager::default()); assert!(registry.entries.lock().unwrap().is_empty());
    std::fs::remove_dir_all(directory).unwrap();
}
#[test]
fn concurrency_and_registry_are_bounded() {
    let registry=AudioStreams::default();
    let first=registry.permits.clone().try_acquire_owned().unwrap();
    let second=registry.permits.clone().try_acquire_owned().unwrap();
    assert!(registry.permits.clone().try_acquire_owned().is_err());
    drop(first); assert!(registry.permits.clone().try_acquire_owned().is_ok()); drop(second);
    let (directory,spaces)=fixture(); std::fs::write(spaces.user_root.join("large.wav"),b"RIFF").unwrap();
    for index in 0..MAX_TOKENS {
        let mut audio=stream(&spaces); audio.window=format!("window-{index}"); registry.insert(audio).unwrap();
    }
    assert!(registry.insert(stream(&spaces)).is_err());
    registry.cleanup(&BackendManager::default()); std::fs::remove_dir_all(directory).unwrap();
}
#[test]
fn traversal_hardlinks_and_unsupported_formats_are_rejected() {
    let (directory,spaces)=fixture(); std::fs::write(directory.join("outside.wav"),b"outside").unwrap();
    std::fs::hard_link(directory.join("outside.wav"),spaces.user_root.join("large.wav")).unwrap();
    assert!(open_stream("main".into(),"main".into(),"session".into(),spaces.user_root.clone(),spaces.clone(),"user".into(),"large.wav".into()).is_err());
    for path in ["../outside.wav","/outside.wav","C:/outside.wav","large.txt",""] {
        assert!(open_stream("main".into(),"main".into(),"session".into(),spaces.user_root.clone(),spaces.clone(),"user".into(),path.into()).is_err());
    }
    std::fs::remove_dir_all(directory).unwrap();
}
#[cfg(windows)]
#[test]
fn read_handle_denies_writes_and_deletion_until_revoke() {
    let (directory,spaces)=fixture(); let path=spaces.user_root.join("large.wav"); std::fs::write(&path,b"RIFF").unwrap();
    let audio=stream(&spaces);
    assert!(OpenOptions::new().write(true).open(&path).is_err()); assert!(std::fs::remove_file(&path).is_err());
    drop(audio); std::fs::write(&path,b"new revision").unwrap(); std::fs::remove_dir_all(directory).unwrap();
}
