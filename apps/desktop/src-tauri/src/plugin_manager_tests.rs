use super::*;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("audioprocess-plugin-manager-{}-{}",
            std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
        fs::create_dir(&root).unwrap();
        Self(fs::canonicalize(root).unwrap())
    }
    fn directory(&self) -> PathBuf { self.0.join("config/plugins") }
    fn data(&self) -> PathBuf { self.0.join("data") }
    fn state(&self) -> PathBuf { self.data().join("plugin-manager/state.json") }
    fn manager(&self) -> (PluginManager,Arc<PluginSnapshot>) {
        let manager = PluginManager::default();
        let snapshot = manager.initialize(&self.directory(),&self.data()).unwrap();
        (manager,snapshot)
    }
    fn source(&self,id: &str,description_size: usize,node_count: usize) -> PathBuf {
        let source = self.0.join(format!("source-{}",NEXT.fetch_add(1,Ordering::Relaxed)));
        fs::create_dir(&source).unwrap();
        write_package(&source,id,description_size,node_count);
        source
    }
    fn installed(&self,folder: &str,id: &str) -> PathBuf {
        let target = self.directory().join(folder);
        fs::create_dir_all(&target).unwrap();
        write_package(&target,id,0,1);
        target
    }
}
impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

fn write_package(root: &Path,id: &str,description_size: usize,node_count: usize) {
    let dll = b"This is deliberately not a DLL: static installation validation must never execute it.";
    let descriptions: Vec<Value> = (0..node_count).map(|index| json!({
        "typeId":format!("{id}.gain_{index}"),"displayName":"Static test gain","description":if description_size == 0 { "Test descriptor".into() } else { "d".repeat(description_size) },
        "execution_domain":"synchronous","stream_role":"none","realtime_role":"none",
        "inputs":[{"id":"audio","type":"Audio","required":true}],"outputs":[{"id":"audio","type":"Audio","required":true}],
        "parameters":[{"id":"gain_db","type":"number","description":"Gain","required":true,"minimum":-24,"maximum":12,"unit":"dB"}],
        "plugin":{"id":id,"implementation_version":"0.1.0","capabilities":[{"id":"ag.whole_sync/1","version":1}],"experimental":true,"production_ready":false}
    })).collect();
    let nodes = encode(&json!({"schema_version":1,"plugin_id":id,"plugin_version":"0.1.0","experimental":true,"production_ready":false,"nodes":descriptions})).unwrap();
    let manifest = json!({"schema_version":1,"plugin_id":id,"plugin_version":"0.1.0","experimental":true,"production_ready":false,
        "abi":{"major":0,"minor":1},"platform":"Windows","architecture":"x86_64","entry":"test.dll","nodes":"nodes.json",
        "capabilities":[{"id":"ag.whole_sync/1","version":1}],"files":[{"path":"test.dll","sha256":sha(dll)},{"path":"nodes.json","sha256":sha(&nodes)}]});
    fs::write(root.join("test.dll"),dll).unwrap();
    fs::write(root.join("nodes.json"),nodes).unwrap();
    fs::write(root.join("manifest.json"),encode(&manifest).unwrap()).unwrap();
}
fn ok_validator(_: &[PathBuf],_: &Path) -> Result<(),String> { Ok(()) }
fn v2_source(fixture: &Fixture) -> PathBuf {
    let source = fixture.source("org.test.v2",0,1);
    let mut manifest: Value = serde_json::from_slice(&fs::read(source.join("manifest.json")).unwrap()).unwrap();
    manifest["schema_version"] = json!(2);
    manifest["files"][0]["role"] = json!("entry");
    manifest["files"][1]["role"] = json!("nodes");
    fs::create_dir_all(source.join("resources/config")).unwrap();
    let resource = b"gain_scale=1.25";
    let dependency = b"Static dependency fixture";
    fs::write(source.join("resources/config/gain-scale.txt"),resource).unwrap();
    fs::write(source.join("org.test.v2.algorithms.dll"),dependency).unwrap();
    manifest["files"].as_array_mut().unwrap().extend([
        json!({"path":"resources/config/gain-scale.txt","sha256":sha(resource),"role":"resource"}),
        json!({"path":"org.test.v2.algorithms.dll","sha256":sha(dependency),"role":"dependency"})]);
    fs::write(source.join("manifest.json"),encode(&manifest).unwrap()).unwrap();
    source
}

#[test]
fn v2_nested_resources_survive_install_disable_enable_and_deferred_uninstall() {
    let fixture = Fixture::new();
    let (manager,_) = fixture.manager();
    let source = v2_source(&fixture);
    let installed = manager.install_validated(&source,true,|roots,_| {
        assert_eq!(fs::read(roots[0].join("resources/config/gain-scale.txt")).unwrap(),b"gain_scale=1.25");
        Ok(())
    }).unwrap();
    let folder = "org.test.v2";
    let resource = fixture.directory().join(folder).join("resources/config/gain-scale.txt");
    let disabled = manager.change_validated(folder,&installed.entries[0].revision,Some(false),ok_validator).unwrap();
    let enabled = manager.change_validated(folder,&disabled.entries[0].revision,Some(true),ok_validator).unwrap();
    assert_eq!(fs::read(&resource).unwrap(),b"gain_scale=1.25");
    let data = fixture.data().join("plugins/org.test.v2/user.txt");
    fs::create_dir_all(data.parent().unwrap()).unwrap();
    fs::write(&data,b"user data").unwrap();
    manager.uninstall(folder,&enabled.entries[0].revision).unwrap();
    let (next,_) = fixture.manager();
    assert!(next.list().unwrap().entries.is_empty());
    assert_eq!(fs::read(data).unwrap(),b"user data");
    let trash = fs::read_dir(fixture.data().join("plugin-manager/trash")).unwrap().next().unwrap().unwrap().path();
    assert_eq!(fs::read(trash.join("package/resources/config/gain-scale.txt")).unwrap(),b"gain_scale=1.25");
}

#[test]
fn v2_hash_tamper_and_undeclared_directory_are_rejected() {
    let fixture = Fixture::new();
    let (manager,_) = fixture.manager();
    let source = v2_source(&fixture);
    fs::write(source.join("resources/config/gain-scale.txt"),b"tampered").unwrap();
    assert!(manager.install_validated(&source,true,ok_validator).is_err());
    let source = v2_source(&fixture);
    fs::create_dir(source.join("undeclared")).unwrap();
    assert!(manager.install_validated(&source,true,ok_validator).is_err());
}

#[test]
fn v2_nested_changes_after_uninstall_request_prevent_backup_move() {
    let fixture = Fixture::new();
    let (manager,_) = fixture.manager();
    let source = v2_source(&fixture);
    let installed = manager.install_validated(&source,true,ok_validator).unwrap();
    manager.uninstall("org.test.v2",&installed.entries[0].revision).unwrap();
    let resource = fixture.directory().join("org.test.v2/resources/config/gain-scale.txt");
    fs::write(&resource,b"changed after uninstall was requested").unwrap();
    let (next,_) = fixture.manager();
    let view = next.list().unwrap();
    assert_eq!(view.entries.len(),1);
    assert!(view.entries[0].pending_uninstall);
    assert!(view.entries[0].error.is_some());
    assert!(resource.exists());
}

#[test]
fn v2_resource_paths_roles_collisions_and_file_caps_are_rejected() {
    let fixture = Fixture::new();
    let source = v2_source(&fixture);
    let manifest: Value = serde_json::from_slice(&fs::read(source.join("manifest.json")).unwrap()).unwrap();
    for path in ["resources/../escape","resources/CON.txt","resources/end.","resources/a/b/c/d","resources/has space","Resources/config/gain-scale.txt"] {
        let mut invalid = manifest.clone();
        invalid["files"][2]["path"] = json!(path);
        assert!(package_members(&invalid,&source,"org.test.v2","test.dll").is_err(),"{path}");
    }
    let mut collision = manifest.clone();
    collision["files"].as_array_mut().unwrap().push(json!({"path":"resources/config/GAIN-SCALE.txt","role":"resource","sha256":"0".repeat(64)}));
    assert!(package_members(&collision,&source,"org.test.v2","test.dll").is_err());
    let mut excessive = manifest.clone();
    excessive["files"] = json!(vec![manifest["files"][0].clone();65]);
    assert!(package_members(&excessive,&source,"org.test.v2","test.dll").is_err());
    let oversized = OpenOptions::new().write(true).open(source.join("resources/config/gain-scale.txt")).unwrap();
    oversized.set_len(DLL_LIMIT+1).unwrap();
    assert!(package_members(&manifest,&source,"org.test.v2","test.dll").is_err());
}
fn byte_members(root: &Path) -> Vec<Vec<u8>> {
    ["manifest.json","nodes.json","test.dll"].into_iter().map(|name| fs::read(root.join(name)).unwrap()).collect()
}

#[test]
fn management_list_has_no_workspace_requirement_and_initial_selection_stays_frozen() {
    let fixture = Fixture::new();
    let (manager,snapshot) = fixture.manager();
    let empty = manager.list().unwrap();
    assert!(empty.entries.is_empty());
    assert!(!empty.restart_required);
    assert!(fixture.directory().is_dir());
    assert!(snapshot.captured_folders().is_empty());
    let source = fixture.source("org.test.first",0,1);
    let original = byte_members(&source);
    let installed = manager.install_validated(&source,true,|roots,operation| {
        assert_eq!(roots.len(),1);
        assert!(roots[0].starts_with(operation));
        assert!(!roots[0].starts_with(fixture.directory()));
        Ok(())
    }).unwrap();
    assert_eq!(installed.entries.len(),1);
    let entry = &installed.entries[0];
    assert_eq!(entry.folder,"org.test.first");
    assert_eq!(entry.plugin_id.as_deref(),Some("org.test.first"));
    assert_eq!(entry.plugin_version.as_deref(),Some("0.1.0"));
    assert_eq!(entry.node_types,vec!["org.test.first.gain_0"]);
    assert!(entry.enabled && entry.can_manage && !entry.active && !entry.pending_uninstall);
    assert!(installed.restart_required);
    assert_eq!(byte_members(&source),original);
    assert_eq!(byte_members(&fixture.directory().join(&entry.folder)),original);
    assert!(snapshot.captured_folders().is_empty());
    let (_,next) = fixture.manager();
    assert!(next.captured_folders().contains("org.test.first"));
}

#[test]
fn revisions_prevent_stale_ui_and_other_instance_changes_and_do_not_reconfigure_active_selection() {
    let fixture = Fixture::new();
    fixture.installed("copied-folder","org.test.first");
    let (first,snapshot) = fixture.manager();
    let (second,_) = fixture.manager();
    let before = first.list().unwrap();
    let old = &before.entries[0];
    assert!(old.active && old.enabled);
    let disabled = second.set_enabled(&old.folder,&old.revision,false).unwrap();
    assert!(!disabled.entries[0].enabled);
    assert!(disabled.entries[0].active);
    assert!(disabled.restart_required);
    assert!(first.uninstall(&old.folder,&old.revision).unwrap_err().contains("stale"));
    let fresh = first.list().unwrap();
    assert_ne!(fresh.entries[0].revision,old.revision);
    let enabled = first.change_validated(&old.folder,&fresh.entries[0].revision,Some(true),ok_validator).unwrap();
    assert!(enabled.entries[0].enabled);
    assert!(!enabled.restart_required);
    assert!(snapshot.captured_folders().contains("copied-folder"));
    let changed = first.list().unwrap();
    fs::write(fixture.directory().join("copied-folder/manifest.json"),b"changed metadata").unwrap();
    assert!(first.uninstall("copied-folder",&changed.entries[0].revision).unwrap_err().contains("stale"));
}

#[test]
fn bad_packages_are_visible_and_can_be_disabled_without_validating_other_bad_packages() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.directory().join("missing-manifest")).unwrap();
    let manager = PluginManager::default();
    assert!(manager.initialize(&fixture.directory(),&fixture.data()).is_err());
    let listed = manager.list().unwrap();
    assert_eq!(listed.entries.len(),1);
    assert!(listed.entries[0].error.is_some());
    assert!(listed.entries[0].can_manage);
    assert!(listed.entries[0].plugin_id.is_none());
    let disabled = manager.set_enabled("missing-manifest",&listed.entries[0].revision,false).unwrap();
    assert!(!disabled.entries[0].enabled);
    let (_,snapshot) = fixture.manager();
    assert!(snapshot.captured_folders().is_empty());
}

#[test]
fn uninstall_is_deferred_and_preserves_complete_backup_plugin_data_audio_and_history() {
    let fixture = Fixture::new();
    let package = fixture.installed("example","org.test.example");
    let original = byte_members(&package);
    fs::create_dir_all(fixture.data().join("plugin-data/workspace/plugin-id")).unwrap();
    fs::create_dir_all(fixture.data().join("run-records")).unwrap();
    fs::write(fixture.data().join("plugin-data/workspace/plugin-id/state.txt"),"keep plugin data").unwrap();
    fs::write(fixture.data().join("run-records/history.json"),"keep history").unwrap();
    fs::write(fixture.0.join("audio.wav"),"keep audio").unwrap();
    let (manager,current) = fixture.manager();
    let listed = manager.list().unwrap();
    let pending = manager.uninstall("example",&listed.entries[0].revision).unwrap();
    assert!(pending.entries[0].pending_uninstall && pending.entries[0].active);
    assert!(!pending.entries[0].enabled);
    assert!(pending.restart_required);
    assert_eq!(byte_members(&package),original);
    let (restarted,snapshot) = fixture.manager();
    let restarted_view = restarted.list().unwrap();
    assert!(restarted_view.entries.is_empty(),"{restarted_view:?}");
    assert!(snapshot.captured_folders().is_empty());
    assert!(current.captured_folders().contains("example"));
    assert!(!package.exists());
    let containers = fs::read_dir(fixture.data().join("plugin-manager/trash")).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
    assert_eq!(containers.len(),1);
    assert_eq!(byte_members(&containers[0].path().join("package")),original);
    assert_eq!(fs::read_to_string(fixture.data().join("plugin-data/workspace/plugin-id/state.txt")).unwrap(),"keep plugin data");
    assert_eq!(fs::read_to_string(fixture.data().join("run-records/history.json")).unwrap(),"keep history");
    assert_eq!(fs::read_to_string(fixture.0.join("audio.wav")).unwrap(),"keep audio");
}

#[test]
fn a_disabled_package_pending_uninstall_still_requires_restart_and_can_be_cancelled_without_enabling() {
    let fixture = Fixture::new();
    fixture.installed("first","org.test.first");
    let (manager,_) = fixture.manager();
    let initial = manager.list().unwrap();
    let disabled = manager.set_enabled("first",&initial.entries[0].revision,false).unwrap();
    let (restarted,_) = fixture.manager();
    assert!(!restarted.list().unwrap().entries[0].active);
    let initial = restarted.list().unwrap();
    let pending = restarted.uninstall("first",&initial.entries[0].revision).unwrap();
    assert!(pending.restart_required);
    assert!(pending.entries[0].pending_uninstall && !pending.entries[0].enabled && !pending.entries[0].active);
    let cancelled = restarted.set_enabled("first",&pending.entries[0].revision,false).unwrap();
    assert!(!cancelled.entries[0].pending_uninstall && !cancelled.entries[0].enabled && !cancelled.entries[0].active);
    assert!(!cancelled.restart_required);
    assert!(fixture.directory().join("first").is_dir());
    assert!(cancelled.entries[0].error.is_none());
    assert!(!disabled.entries[0].enabled);
}

#[test]
fn unknown_resources_untrusted_sources_hash_changes_links_and_duplicate_ids_are_refused() {
    let fixture = Fixture::new();
    let (manager,_) = fixture.manager();
    let source = fixture.source("org.test.first",0,1);
    assert!(manager.install_validated(&source,false,ok_validator).is_err());
    fs::write(source.join("extra.key"),"never copied").unwrap();
    assert!(manager.install_validated(&source,true,ok_validator).is_err());
    fs::remove_file(source.join("extra.key")).unwrap();
    fs::write(source.join("test.dll"),"changed source").unwrap();
    assert!(manager.install_validated(&source,true,ok_validator).is_err());
    assert!(!fixture.directory().join("org.test.first").exists());
    write_package(&source,"org.test.first",0,1);
    manager.install_validated(&source,true,ok_validator).unwrap();
    let original = byte_members(&fixture.directory().join("org.test.first"));
    assert!(manager.install_validated(&source,true,ok_validator).is_err());
    assert_eq!(byte_members(&fixture.directory().join("org.test.first")),original);
    let linked = fixture.source("org.test.link",0,1);
    fs::hard_link(linked.join("test.dll"),fixture.0.join("outside-linked.dll")).unwrap();
    assert!(manager.install_validated(&linked,true,ok_validator).is_err());
    for folder in ["../escape","a/b","a\\b","C:",".","CON"] {
        assert!(manager.set_enabled(folder,"any",false).is_err());
        assert!(manager.uninstall(folder,"any").is_err());
    }
}

#[test]
fn invalid_state_is_never_silently_replaced_or_reset() {
    let fixture = Fixture::new();
    let (manager,_) = fixture.manager();
    fs::write(fixture.state(),b"{broken state; do not reset").unwrap();
    assert!(manager.list().is_err());
    let next = PluginManager::default();
    assert!(next.initialize(&fixture.directory(),&fixture.data()).is_err());
    assert!(next.list().is_err());
    assert_eq!(fs::read(fixture.state()).unwrap(),b"{broken state; do not reset");
    let source = fixture.source("org.test.source",0,1);
    assert!(manager.install_validated(&source,true,ok_validator).is_err());
    assert!(!fixture.directory().join("org.test.source").exists());
}

#[test]
fn all_next_enabled_packages_are_validated_before_install_or_enable() {
    let fixture = Fixture::new();
    fixture.installed("first","org.test.first");
    fixture.installed("second","org.test.second");
    let (manager,_) = fixture.manager();
    let source = fixture.source("org.test.third",0,1);
    let failed = manager.install_validated(&source,true,|roots,_| {
        assert_eq!(roots.len(),3);
        assert!(roots.iter().any(|root| root == &fixture.directory().join("first")));
        assert!(roots.iter().any(|root| root == &fixture.directory().join("second")));
        Err("Complete catalog exceeds its static byte budget".into())
    });
    assert!(failed.is_err());
    assert!(!fixture.directory().join("org.test.third").exists());
    let listed = manager.list().unwrap();
    let second = listed.entries.iter().find(|entry| entry.folder == "second").unwrap();
    let disabled = manager.set_enabled("second",&second.revision,false).unwrap();
    let second = disabled.entries.iter().find(|entry| entry.folder == "second").unwrap();
    assert!(manager.change_validated("second",&second.revision,Some(true),|roots,_| {
        assert_eq!(roots.len(),2);
        Err("The complete catalog conflicts".into())
    }).is_err());
    assert!(!manager.list().unwrap().entries.iter().find(|entry| entry.folder == "second").unwrap().enabled);
}

#[test]
fn reserve_or_final_state_write_failure_never_activates_a_failed_install() {
    for failing_write in [1,2] {
        let fixture = Fixture::new();
        let (manager,original_snapshot) = fixture.manager();
        let source = fixture.source("org.test.first",0,1);
        let original = byte_members(&source);
        FAIL_STATE_WRITE.with(|counter| counter.set(Some(failing_write)));
        assert!(manager.install_validated(&source,true,ok_validator).is_err());
        assert_eq!(byte_members(&source),original);
        assert!(original_snapshot.captured_folders().is_empty());
        let listed = manager.list().unwrap();
        if failing_write == 1 { assert!(listed.entries.is_empty()); }
        else {
            assert_eq!(listed.entries.len(),1);
            assert!(!listed.entries[0].enabled && !listed.entries[0].active);
            assert_eq!(byte_members(&fixture.directory().join("org.test.first")),original);
        }
        let (_,next_snapshot) = fixture.manager();
        assert!(next_snapshot.captured_folders().is_empty());
    }
}

#[test]
fn at_most_sixteen_packages_can_be_enabled_for_next_startup() {
    let fixture = Fixture::new();
    for index in 0..16 { fixture.installed(&format!("folder-{index:02}"),&format!("org.test.p{index:02}")); }
    let (manager,_) = fixture.manager();
    let source = fixture.source("org.test.seventeenth",0,1);
    assert!(manager.install_validated(&source,true,ok_validator).unwrap_err().contains("16"));
    assert!(!fixture.directory().join("org.test.seventeenth").exists());
}

#[cfg(windows)]
#[test]
fn atomic_folder_move_itself_refuses_a_live_dll_pin_and_succeeds_after_the_pin_closes() {
    use std::os::windows::fs::OpenOptionsExt;
    let fixture = Fixture::new();
    let source = fixture.installed("first","org.test.first");
    let destination = fixture.0.join("new-backup");
    let held = OpenOptions::new().read(true).share_mode(1).open(source.join("test.dll")).unwrap();
    assert!(move_new(&source,&destination).is_err(),"A production-style DLL pin must prevent the parent rename even after our probes close");
    assert!(source.is_dir());
    assert!(!destination.exists());
    drop(held);
    move_new(&source,&destination).unwrap();
    assert!(!source.exists());
    assert!(destination.join("test.dll").is_file());
}

#[cfg(windows)]
#[test]
fn another_instance_pinning_a_package_leaves_uninstall_pending_and_excludes_it_from_startup() {
    use std::os::windows::fs::OpenOptionsExt;
    let fixture = Fixture::new();
    let package = fixture.installed("first","org.test.first");
    let (manager,_) = fixture.manager();
    let listed = manager.list().unwrap();
    manager.uninstall("first",&listed.entries[0].revision).unwrap();
    let held = OpenOptions::new().read(true).share_mode(1).open(package.join("test.dll")).unwrap();
    let (next,snapshot) = fixture.manager();
    assert!(package.exists());
    assert!(snapshot.captured_folders().is_empty());
    let pending = next.list().unwrap();
    assert!(pending.entries[0].pending_uninstall && !pending.entries[0].active);
    assert!(pending.entries[0].error.is_some());
    assert!(!pending.warnings.is_empty());
    drop(held);
    let (after_unlock,_) = fixture.manager();
    let after_unlock_view = after_unlock.list().unwrap();
    assert!(after_unlock_view.entries.is_empty(),"{after_unlock_view:?}");
    assert!(!package.exists());
}

#[cfg(windows)]
fn executable() -> PathBuf {
    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    [project.join("build/Debug/control-cli.exe"),project.join("build/Release/control-cli.exe")]
        .into_iter().find(|path| path.is_file()).expect("Build the plugin-aware fixed sidecar before integration tests")
}

#[cfg(windows)]
#[test]
fn real_static_capabilities_accepts_a_nonexecutable_dll_without_loading_or_running_it() {
    let fixture = Fixture::new();
    let (manager,_) = fixture.manager();
    let source = fixture.source("org.test.static",0,1);
    let result = manager.install_validated(&source,true,|roots,operation|
        validate_roots_with_executable(roots,operation,&executable())).unwrap();
    assert!(result.entries[0].enabled && !result.entries[0].active);
    assert_eq!(fs::read(fixture.directory().join("org.test.static/test.dll")).unwrap(),
        b"This is deliberately not a DLL: static installation validation must never execute it.");
}

#[cfg(windows)]
#[test]
fn real_static_validation_rejects_an_individually_valid_package_that_overflows_the_complete_catalog() {
    let fixture = Fixture::new();
    let (manager,_) = fixture.manager();
    let first = fixture.source("org.test.first",1700,8);
    let second = fixture.source("org.test.second",1700,8);
    manager.install_validated(&first,true,|roots,operation| validate_roots_with_executable(roots,operation,&executable())).unwrap();
    let result = manager.install_validated(&second,true,|roots,operation| validate_roots_with_executable(roots,operation,&executable()));
    assert!(result.is_err());
    let view = manager.list().unwrap();
    assert_eq!(view.entries.len(),1);
    assert_eq!(view.entries[0].plugin_id.as_deref(),Some("org.test.first"));
    assert!(!fixture.directory().join("org.test.second").exists());
    assert!(second.join("test.dll").is_file());
}
