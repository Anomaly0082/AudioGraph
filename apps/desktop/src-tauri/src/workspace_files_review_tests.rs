//! Independent regression cases. These tests never call the real Recycle Bin.
use super::*;
struct Fixture { root: PathBuf, spaces: ToolWorkspaces }
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("manual-files-review-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
        fs::create_dir(&root).unwrap();
        let user_root=root.join("user"); let ai_root=root.join("ai");
        fs::create_dir(&user_root).unwrap(); fs::create_dir(&ai_root).unwrap();
        Self {root,spaces:ToolWorkspaces{user_root,ai_root}}
    }
}
impl Drop for Fixture {fn drop(&mut self){let _=fs::remove_dir_all(&self.root);}}

#[cfg(windows)]
#[test]
fn canonical_shell_conversion_resolves_same_fixture_without_recycling() {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::System::Com::{CoInitializeEx,CoUninitialize,COINIT_APARTMENTTHREADED,IBindCtx};
    use windows::Win32::UI::Shell::{IShellItem,SHCreateItemFromParsingName};
    let f=Fixture::new();let folder=f.spaces.user_root.join("中文 文件夹");
    fs::create_dir(&folder).unwrap();let file=folder.join("音频 文件.wav");fs::write(&file,b"fixture").unwrap();
    unsafe {
        CoInitializeEx(None,COINIT_APARTMENTTHREADED).ok().unwrap();
        struct Com;impl Drop for Com {fn drop(&mut self){unsafe{CoUninitialize()}}}let _com=Com;
        for path in [&folder,&file] {
            let canonical=fs::canonicalize(path).unwrap();let shell=shell_file_path(&canonical).unwrap();
            assert_eq!(fs::canonicalize(&shell).unwrap(),canonical);
            let wide:Vec<u16>=shell.as_os_str().encode_wide().chain(Some(0)).collect();
            let _:IShellItem=SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()),None::<&IBindCtx>).unwrap();
            assert!(path.exists());
        }
    }
    assert_eq!(fs::read(file).unwrap(),b"fixture");
}

#[test]
fn mutations_reject_all_root_aliases_and_invalid_names() {
    let f=Fixture::new();
    for name in ["", ".", "..", "a/..", "/", "C:/x", "a\\b", "x:stream", "NUL.txt", "com1.log", "lpt9", "tail ", "tail.", "a//b", "a\0b"] {
        assert!(mkdir(&f.spaces,"user",name).is_err(),"accepted {name:?}");
        assert!(transfer(&f.spaces,"user",name,"target",true).is_err(),"accepted source {name:?}");
    }
    assert!(mkdir(&f.spaces,"other","folder").is_err());
    assert_eq!(fs::read_dir(&f.spaces.user_root).unwrap().count(),0);
}

#[test]
fn destination_within_source_is_rejected_before_creation() {
    let f=Fixture::new(); fs::create_dir(f.spaces.user_root.join("source")).unwrap();
    fs::write(f.spaces.user_root.join("source/keep"),b"original").unwrap();
    for moving in [false,true] {
        assert!(transfer(&f.spaces,"user","source","source/child",moving).is_err());
    }
    assert!(transfer(&f.spaces,"user","source","source",true).is_err());
    #[cfg(windows)]
    {
        let copied=transfer(&f.spaces,"user","source","source",false).unwrap();
        assert_eq!(copied["path"],"source 副本");
        assert_eq!(fs::read(f.spaces.user_root.join("source 副本/keep")).unwrap(),b"original");
    }
    assert_eq!(fs::read_dir(f.spaces.user_root.join("source")).unwrap().count(),1);
    assert_eq!(fs::read(f.spaces.user_root.join("source/keep")).unwrap(),b"original");
}

#[test]
fn hardlink_nested_in_directory_blocks_entire_operation() {
    let f=Fixture::new(); fs::create_dir(f.spaces.user_root.join("source")).unwrap();
    let external=f.root.join("outside"); fs::write(&external,b"private").unwrap();
    fs::hard_link(&external,f.spaces.user_root.join("source/link")).unwrap();
    for moving in [false,true] {assert!(transfer(&f.spaces,"user","source","copy",moving).is_err());}
    assert!(!f.spaces.user_root.join("copy").exists());
    assert_eq!(fs::read(external).unwrap(),b"private");
}

#[test]
fn import_preflights_later_conflicts_before_first_write() {
    let f=Fixture::new(); let first=f.root.join("first");let second=f.root.join("second");
    fs::write(&first,b"first").unwrap();fs::write(&second,b"second").unwrap();
    fs::write(f.spaces.user_root.join("second"),b"existing").unwrap();
    assert!(import_files(&f.spaces,"user","",&[first.clone(),second.clone()]).is_err());
    assert!(!f.spaces.user_root.join("first").exists());
    assert_eq!(fs::read(f.spaces.user_root.join("second")).unwrap(),b"existing");
    assert_eq!(fs::read(first).unwrap(),b"first");assert_eq!(fs::read(second).unwrap(),b"second");
}

#[test]
fn missing_parent_never_creates_intermediate_directories() {
    let f=Fixture::new();fs::write(f.spaces.user_root.join("file"),b"data").unwrap();
    assert!(mkdir(&f.spaces,"user","missing/child").is_err());
    assert!(transfer(&f.spaces,"user","file","missing/copy",false).is_err());
    assert!(!f.spaces.user_root.join("missing").exists());
}

#[cfg(windows)]
#[test]
fn native_file_import_copies_into_selected_folder_and_preserves_source() {
    let f=Fixture::new(); let external=f.root.join("外部音频.wav");
    fs::write(&external,b"audio bytes").unwrap();
    fs::create_dir(f.spaces.user_root.join("selected")).unwrap();
    let reply=import_files(&f.spaces,"user","selected",&[external.clone()]).unwrap();
    assert_eq!(reply["imported"][0],"外部音频.wav");
    assert_eq!(fs::read(f.spaces.user_root.join("selected/外部音频.wav")).unwrap(),b"audio bytes");
    assert_eq!(fs::read(external).unwrap(),b"audio bytes");
    assert!(!f.spaces.user_root.join("外部音频.wav").exists());
}

#[cfg(windows)]
#[test]
fn published_copy_and_move_never_overwrite_existing_files() {
    let f=Fixture::new();fs::write(f.spaces.user_root.join("source"),b"new").unwrap();
    fs::write(f.spaces.user_root.join("target"),b"keep").unwrap();
    for moving in [false,true] {assert!(transfer(&f.spaces,"user","source","target",moving).is_err());}
    assert_eq!(fs::read(f.spaces.user_root.join("source")).unwrap(),b"new");
    assert_eq!(fs::read(f.spaces.user_root.join("target")).unwrap(),b"keep");
    transfer(&f.spaces,"user","source","copy",false).unwrap();
    transfer(&f.spaces,"user","copy","moved",true).unwrap();
    assert_eq!(fs::read(f.spaces.user_root.join("moved")).unwrap(),b"new");
    assert!(!f.spaces.user_root.join("copy").exists());
    assert!(!fs::read_dir(&f.spaces.user_root).unwrap().any(|e|e.unwrap().file_name().to_string_lossy().starts_with(".manual-tmp-")));
}
