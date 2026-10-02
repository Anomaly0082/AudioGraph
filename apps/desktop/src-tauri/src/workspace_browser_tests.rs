use super::*;
use std::sync::atomic::{AtomicU64,Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn fixture() -> (std::path::PathBuf,ToolWorkspaces) {
    let directory = std::env::temp_dir().join(format!("audioprocess-file-browser-{}-{}",
        std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
    std::fs::create_dir_all(directory.join("user")).unwrap();
    let spaces = ToolWorkspaces::new_read_only(&directory.join("user"),&directory.join("data")).unwrap();
    (directory,spaces)
}

#[test]
fn browsing_a_fresh_workspace_creates_no_app_data_or_ai_directories() {
    let (directory,spaces) = fixture();
    std::fs::write(spaces.user_root.join("note.txt"),"user data").unwrap();
    let listing = list_directory(&spaces,"user","",0,100).unwrap();
    assert_eq!(listing.entries.len(),1);
    assert_eq!(preview_text(&spaces,"user","note.txt").unwrap().text,"user data");
    let ai = list_directory(&spaces,"ai","",0,100).unwrap();
    assert!(ai.entries.is_empty());
    assert_eq!(ai.total,Some(0));
    assert!(!ai.partial);
    assert!(!directory.join("data").exists());
    assert!(!spaces.ai_root.exists());
    let prepared = ToolWorkspaces::new(&directory.join("user"),&directory.join("data")).unwrap();
    assert_eq!(spaces.ai_root,prepared.ai_root);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn directory_pages_sort_consistently_and_report_partial_scan_budgets() {
    let (directory,spaces) = fixture();
    std::fs::create_dir(spaces.user_root.join("z-folder")).unwrap();
    std::fs::write(spaces.user_root.join("b.txt"),"bb").unwrap();
    std::fs::write(spaces.user_root.join("A.txt"),"a").unwrap();
    let first = list_directory(&spaces,"user","",0,2).unwrap();
    assert_eq!(first.entries.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(),vec!["z-folder","A.txt"]);
    assert_eq!(first.next_offset,Some(2));
    assert_eq!(first.total,Some(3));
    assert!(!first.partial);
    assert_eq!(first.entries[0].kind,"directory");
    assert_eq!(first.entries[0].bytes,None);
    assert!(first.entries[1].modified_at_ms.is_some());
    let second = list_directory(&spaces,"user","",2,2).unwrap();
    assert_eq!(second.entries.len(),1);
    assert_eq!(second.entries[0].path,"b.txt");
    assert_eq!(second.entries[0].bytes,Some(2));
    assert_eq!(second.next_offset,None);
    let partial = list_directory_bounded(&spaces,"user","",0,100,1,Instant::now()+SCAN_TIME).unwrap();
    assert!(partial.partial);
    assert_eq!(partial.total,None);
    assert_eq!(partial.entries.len(),1);
    assert!(!partial.warnings.is_empty());
    let timed = list_directory_bounded(&spaces,"user","",0,100,100,Instant::now()).unwrap();
    assert!(timed.partial);
    assert!(timed.entries.is_empty());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn text_prefix_preserves_utf8_boundary_and_complete_flag_without_executing_json() {
    let (directory,spaces) = fixture();
    let path = spaces.user_root.join("large.txt");
    let content = format!("{}好后文","a".repeat(TEXT_LIMIT-1));
    std::fs::write(&path,content.as_bytes()).unwrap();
    let preview = preview_text(&spaces,"user","large.txt").unwrap();
    assert!(preview.truncated);
    assert_eq!(preview.text.len(),TEXT_LIMIT-1);
    assert_eq!(preview.bytes,content.len() as u64);
    std::fs::write(spaces.user_root.join("exact.txt"),"x".repeat(TEXT_LIMIT)).unwrap();
    let exact = preview_text(&spaces,"user","exact.txt").unwrap();
    assert!(!exact.truncated);
    assert_eq!(exact.text.len(),TEXT_LIMIT);
    std::fs::write(spaces.user_root.join("broken.workflow.json"),"{broken JSON").unwrap();
    assert_eq!(preview_text(&spaces,"user","broken.workflow.json").unwrap().text,"{broken JSON");
    for (name,bytes) in [("binary.bin",vec![0,1,2]),("invalid-utf8.bin",vec![0xff,0xfe])] {
        std::fs::write(spaces.user_root.join(name),bytes).unwrap();
        assert!(preview_text(&spaces,"user",name).is_err());
    }
    assert!(!directory.join("data").exists());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn audio_snapshots_are_bounded_read_only_bytes_and_not_dsp_format_validation() {
    let (directory,spaces) = fixture();
    for name in ["test.WAV","test.mp3","test.ogg","test.oga","test.flac","test.m4a","test.aac","test.webm"] {
        let bytes = b"local snapshot; browser decides whether it can decode";
        std::fs::write(spaces.user_root.join(name),bytes).unwrap();
        assert_eq!(preview_audio(&spaces,"user",name).unwrap(),bytes);
        assert_eq!(std::fs::read(spaces.user_root.join(name)).unwrap(),bytes);
    }
    std::fs::write(spaces.user_root.join("not-audio.txt"),"text").unwrap();
    assert!(preview_audio(&spaces,"user","not-audio.txt").is_err());
    let large = File::create(spaces.user_root.join("large.wav")).unwrap();
    large.set_len(AUDIO_LIMIT as u64+1).unwrap();
    drop(large);
    assert!(preview_audio(&spaces,"user","large.wav").unwrap_err().contains("32 MiB"));
    let body = tauri::ipc::IpcResponse::body(tauri::ipc::Response::new(vec![1_u8,2,3])).unwrap();
    assert!(matches!(body,tauri::ipc::InvokeResponseBody::Raw(bytes) if bytes == vec![1,2,3]));
    assert!(!directory.join("data").exists());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn path_and_page_attacks_never_expand_the_workspace() {
    let (directory,spaces) = fixture();
    std::fs::write(directory.join("outside.txt"),"outside").unwrap();
    for path in ["../outside.txt","C:/outside.txt","/outside.txt","dir/../outside.txt","a\\b","CON","."] {
        assert!(list_directory(&spaces,"user",path,0,100).is_err(),"{path}");
        assert!(preview_text(&spaces,"user",path).is_err(),"{path}");
    }
    assert!(list_directory(&spaces,"unknown","",0,100).is_err());
    assert!(preview_text(&spaces,"user","").is_err());
    assert!(preview_text(&spaces,"user","missing.txt").is_err());
    assert!(list_directory(&spaces,"user","",0,0).is_err());
    assert!(list_directory(&spaces,"user","",0,PAGE_LIMIT+1).is_err());
    assert!(list_directory(&spaces,"user","",SCAN_LIMIT+1,100).is_err());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn linked_or_unreadable_entries_do_not_break_directory_browsing() {
    let (directory,spaces) = fixture();
    std::fs::write(directory.join("outside.txt"),"outside secret").unwrap();
    std::fs::hard_link(directory.join("outside.txt"),spaces.user_root.join("linked.txt")).unwrap();
    std::fs::write(spaces.user_root.join("normal.txt"),"normal").unwrap();
    let listing = list_directory(&spaces,"user","",0,100).unwrap();
    assert_eq!(listing.entries.len(),2);
    let linked = listing.entries.iter().find(|entry| entry.name == "linked.txt").unwrap();
    assert_eq!(linked.kind,"unsupported");
    assert_eq!(linked.bytes,None);
    assert!(linked.message.is_some());
    assert!(preview_text(&spaces,"user","linked.txt").is_err());
    assert_eq!(preview_text(&spaces,"user","normal.txt").unwrap().text,"normal");
    std::fs::remove_dir_all(directory).unwrap();
}

#[cfg(windows)]
#[test]
fn opened_file_handle_rejects_outside_workspace_even_if_a_precheck_was_valid() {
    let (directory,spaces) = fixture();
    std::fs::write(directory.join("outside.wav"),"outside secret").unwrap();
    let outside = File::open(directory.join("outside.wav")).unwrap();
    assert!(!opened_file_within_workspace(&outside,&spaces.user_root));
    std::fs::write(spaces.user_root.join("inside.wav"),"inside").unwrap();
    let inside = File::open(spaces.user_root.join("inside.wav")).unwrap();
    assert!(opened_file_within_workspace(&inside,&spaces.user_root));
    drop(outside);
    drop(inside);
    std::fs::remove_dir_all(directory).unwrap();
}
