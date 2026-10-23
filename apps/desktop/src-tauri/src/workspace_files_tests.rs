use super::*;
struct Fixture(PathBuf,ToolWorkspaces);
impl Fixture{fn new()->Self{let root=std::env::temp_dir().join(format!("manual-files-test-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));fs::create_dir(&root).unwrap();let user=root.join("user");let ai=root.join("ai");fs::create_dir(&user).unwrap();fs::create_dir(&ai).unwrap();Self(root,ToolWorkspaces{user_root:user,ai_root:ai})}}
impl Drop for Fixture{fn drop(&mut self){let _=fs::remove_dir_all(&self.0);}}
#[test]fn rejects_root_traversal_devices_ads_and_conflicts(){let f=Fixture::new();for name in ["","../x","CON","x:stream","x/../y","tail."]{assert!(mkdir(&f.1,"user",name).is_err(),"{name}");}mkdir(&f.1,"user","folder").unwrap();assert!(mkdir(&f.1,"user","folder").is_err());assert!(transfer(&f.1,"user","","other",true).is_err());}
#[test]fn preflight_rejects_hardlinks_and_deep_trees(){let f=Fixture::new();fs::write(f.1.user_root.join("file"),b"test").unwrap();fs::hard_link(f.1.user_root.join("file"),f.1.user_root.join("link")).unwrap();assert!(transfer(&f.1,"user","file","copy",false).is_err());assert!(!f.1.user_root.join("copy").exists());let mut dir=f.1.user_root.join("deep");fs::create_dir(&dir).unwrap();for _ in 0..17{dir=dir.join("d");fs::create_dir(&dir).unwrap();}assert!(inspect_tree(&f.1.user_root.join("deep")).is_err());}
#[cfg(windows)]
#[test]fn copies_and_moves_without_overwriting(){let f=Fixture::new();mkdir(&f.1,"user","source").unwrap();fs::write(f.1.user_root.join("source/file"),b"hello").unwrap();transfer(&f.1,"user","source","copy",false).unwrap();assert_eq!(fs::read(f.1.user_root.join("copy/file")).unwrap(),b"hello");assert!(transfer(&f.1,"user","source","copy",true).is_err());transfer(&f.1,"user","copy","renamed",true).unwrap();assert!(!f.1.user_root.join("copy").exists());assert!(f.1.user_root.join("source").exists());}
#[test]fn external_import_rejects_folders_and_duplicate_names(){let f=Fixture::new();assert!(import_files(&f.1,"user","",&[f.1.ai_root.clone()]).is_err());fs::write(f.1.ai_root.join("file"),b"hello").unwrap();assert!(import_files(&f.1,"user","",&[f.1.ai_root.join("file"),f.1.ai_root.join("file")]).is_err());assert!(!f.1.user_root.join("file").exists());}
#[cfg(windows)]
#[test]
#[ignore="Opt-in only: recycles a generated temporary fixture through the real Windows Recycle Bin"]
fn native_recycle_fixture(){
    let f=Fixture::new();
    fs::write(f.1.user_root.join("fixture"),b"temporary test fixture").unwrap();
    recycle(&f.1,"user","fixture").unwrap();
    assert!(!f.1.user_root.join("fixture").exists());
    fs::create_dir(f.1.user_root.join("folder-fixture")).unwrap();
    fs::write(f.1.user_root.join("folder-fixture/child"),b"temporary child fixture").unwrap();
    recycle(&f.1,"user","folder-fixture").unwrap();
    assert!(!f.1.user_root.join("folder-fixture").exists());
}
#[test]
fn import_rejects_more_than_native_grant_limit(){
    let f=Fixture::new();
    assert!(import_files(&f.1,"user","",&vec![f.0.join("not-opened");33]).is_err());
    assert_eq!(fs::read_dir(&f.1.user_root).unwrap().count(),0);
}
#[cfg(windows)]
#[test]
#[ignore="Opt-in: only generated canonical-path fixtures enter the Windows Recycle Bin"]
fn native_recycle_canonical_workspace_regression() {
    let f = Fixture::new();
    // Production uses canonical roots, unlike the original handcrafted fixture.
    let spaces = ToolWorkspaces::new(&f.1.user_root, &f.0.join("app-data")).unwrap();
    assert!(spaces.user_root.to_string_lossy().starts_with(r"\\?\"));
    for space in ["user", "ai"] {
        let root = if space == "user" { &spaces.user_root } else { &spaces.ai_root };
        fs::create_dir(root.join("中文 文件夹")).unwrap();
        fs::write(root.join("中文 文件夹/processed.wav"), b"synthetic fixture, not user audio").unwrap();
        recycle(&spaces, space, "中文 文件夹/processed.wav").unwrap();
        assert!(!root.join("中文 文件夹/processed.wav").exists());
        fs::write(root.join("中文 文件夹/child.txt"), b"child fixture").unwrap();
        recycle(&spaces, space, "中文 文件夹").unwrap();
        assert!(!root.join("中文 文件夹").exists());
    }
}
#[cfg(windows)]
#[test]
fn recycle_sink_refuses_permanent_delete_without_touching_files(){
    let sink:IFileOperationProgressSink=RecycleOnly.into();
    unsafe{
        assert!(sink.PreDeleteItem(0,None::<&IShellItem>).is_err());
        assert!(sink.PreDeleteItem(0x80,None::<&IShellItem>).is_ok());
    }
}

#[cfg(windows)]
#[test]
fn shell_paths_convert_disk_and_unc_without_accepting_devices() {
    for (source, expected) in [
        (r"\\?\C:\测试 文件\processed.wav", r"C:\测试 文件\processed.wav"),
        (r"C:\normal\file.txt", r"C:\normal\file.txt"),
        (r"\\?\UNC\server\share\中文\file.txt", r"\\server\share\中文\file.txt"),
        (r"\\server\share\file.txt", r"\\server\share\file.txt"),
    ] {
        assert_eq!(shell_file_path(Path::new(source)).unwrap(), PathBuf::from(expected));
    }
    for source in [r"relative.txt", r"C:file.txt", r"\\.\pipe\anything", r"\\?\Volume{123}\file", r"\\?\C:\dir\..\file"] {
        assert!(shell_file_path(Path::new(source)).is_err(), "{source}");
    }
}

#[cfg(windows)]
#[test]
fn same_directory_copy_allocates_unique_siblings_and_preserves_sources() {
    let f = Fixture::new();
    fs::write(f.1.user_root.join("录音.wav"), b"source").unwrap();
    fs::write(f.1.user_root.join("录音 副本.wav"), b"keep").unwrap();
    let reply = transfer(&f.1, "user", "录音.wav", "录音.wav", false).unwrap();
    assert_eq!(reply["path"], "录音 副本 (2).wav");
    assert_eq!(fs::read(f.1.user_root.join("录音 副本.wav")).unwrap(), b"keep");
    assert_eq!(fs::read(f.1.user_root.join("录音 副本 (2).wav")).unwrap(), b"source");
    assert_eq!(fs::read(f.1.user_root.join("录音.wav")).unwrap(), b"source");
    assert!(transfer(&f.1, "user", "录音.wav", "录音.wav", true).is_err());
    fs::create_dir(f.1.user_root.join("folder.v1")).unwrap();
    fs::write(f.1.user_root.join("folder.v1/file"), b"child").unwrap();
    let folder = transfer(&f.1, "user", "folder.v1", "folder.v1", false).unwrap();
    assert_eq!(folder["path"], "folder.v1 副本");
    assert_eq!(fs::read(f.1.user_root.join("folder.v1 副本/file")).unwrap(), b"child");
}
