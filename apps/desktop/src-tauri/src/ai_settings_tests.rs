use super::*;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let serial = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "audioprocess_ai_settings_test_{}_{}_{}",
            std::process::id(), nonce, serial
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn settings_path(&self) -> PathBuf { self.0.join("ai-settings.json") }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        // Only entries in this test-owned, freshly created directory are removed.
        if let Ok(entries) = fs::read_dir(&self.0) {
            for entry in entries.flatten() {
                let path = entry.path();
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    let _ = fs::remove_dir(&path);
                } else {
                    let _ = fs::remove_file(&path);
                }
            }
        }
        let _ = fs::remove_dir(&self.0);
    }
}

fn config(base_url: &str, model: &str, api_key: &str) -> AiConfig {
    AiConfig { base_url: base_url.into(), model: model.into(), api_key: api_key.into() }
}

fn assert_config(actual: &AiConfig, expected: &AiConfig) {
    assert_eq!(actual.base_url, expected.base_url);
    assert_eq!(actual.model, expected.model);
    assert!(actual.api_key == expected.api_key, "API Key differs");
}

fn assert_rejected_without_key(error: &str) {
    assert!(!error.is_empty());
    assert!(!error.contains("dummy-test-key"));
}

#[test]
fn missing_file_then_save_restore_replace_and_clear() {
    let dir = TestDir::new();
    let path = dir.settings_path();
    let first = config("https://example.test/v1", "first-model", "dummy-test-key");
    let second = config("http://127.0.0.1:12345/v1", "local-model", "");

    assert!(SettingsStore::default().load(&path).unwrap().is_none());
    assert!(!path.exists());

    SettingsStore::default().save(&path, &first).unwrap();
    let disk: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(disk["schema_version"], 1);
    assert_eq!(disk["config"]["baseUrl"], first.base_url);
    assert_eq!(disk["config"]["model"], first.model);
    assert!(disk["config"]["apiKey"] == first.api_key, "persisted API Key differs");
    assert_eq!(disk.as_object().unwrap().len(), 2);

    let restored = SettingsStore::default().load(&path).unwrap().unwrap();
    assert_config(&restored, &first);
    SettingsStore::default().save(&path, &second).unwrap();
    let replaced = SettingsStore::default().load(&path).unwrap().unwrap();
    assert_config(&replaced, &second);

    SettingsStore::default().clear(&path).unwrap();
    assert!(SettingsStore::default().load(&path).unwrap().is_none());
    SettingsStore::default().clear(&path).unwrap();
}

#[test]
fn bad_file_contents_are_rejected_without_echoing_secret() {
    let dir = TestDir::new();
    let path = dir.settings_path();
    let store = SettingsStore::default();
    let invalid = [
        br#"{"schema_version":1,"config":{"baseUrl":"https://example.test/v1","model":"m","apiKey":"dummy-test-key"}"#.as_slice(),
        br#"{"schema_version":2,"config":{"baseUrl":"https://example.test/v1","model":"m","apiKey":"dummy-test-key"}}"#.as_slice(),
        br#"{"schema_version":1,"schema_version":1,"config":{"baseUrl":"https://example.test/v1","model":"m","apiKey":"dummy-test-key"}}"#.as_slice(),
        br#"{"schema_version":1,"config":{"baseUrl":"https://example.test/v1","model":"m","model":"other","apiKey":"dummy-test-key"}}"#.as_slice(),
        br#"{"schema_version":1,"config":{"baseUrl":"https://example.test/v1","model":"m","apiKey":"dummy-test-key"},"extra":true}"#.as_slice(),
    ];
    for bytes in invalid {
        fs::write(&path, bytes).unwrap();
        let error = match store.load(&path) {
            Err(error) => error,
            Ok(_) => panic!("invalid settings file was accepted"),
        };
        assert_rejected_without_key(&error);
    }

    fs::write(&path, vec![b' '; 16 * 1024 + 1]).unwrap();
    let error = match store.load(&path) {
        Err(error) => error,
        Ok(_) => panic!("oversized settings file was accepted"),
    };
    assert_rejected_without_key(&error);
}

#[test]
fn invalid_input_never_replaces_saved_file() {
    let dir = TestDir::new();
    let path = dir.settings_path();
    let store = SettingsStore::default();
    let good = config("https://example.test/v1", "good-model", "dummy-test-key");
    store.save(&path, &good).unwrap();
    let original = fs::read(&path).unwrap();

    for bad in [
        config("http://example.test/v1", "good-model", "dummy-test-key"),
        config("https://example.test/v1", "", "dummy-test-key"),
        config("https://example.test/v1", "good-model", "dummy-test-key\ninvalid"),
    ] {
        let error = store.save(&path, &bad).unwrap_err();
        assert_rejected_without_key(&error);
        assert!(fs::read(&path).unwrap() == original, "invalid save changed existing settings");
    }
    assert_config(&SettingsStore::default().load(&path).unwrap().unwrap(), &good);
}

#[test]
fn directories_are_not_treated_as_settings_files() {
    let dir = TestDir::new();
    let path = dir.settings_path();
    fs::create_dir(&path).unwrap();
    let store = SettingsStore::default();
    let valid = config("https://example.test/v1", "model", "dummy-test-key");
    assert!(store.load(&path).is_err());
    assert!(store.save(&path, &valid).is_err());
    assert!(store.clear(&path).is_err());
    assert!(path.is_dir());
}

#[cfg(windows)]
#[test]
fn failed_replace_preserves_old_file_and_removes_temporary_file() {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    let dir = TestDir::new();
    let path = dir.settings_path();
    let store = SettingsStore::default();
    store.save(&path, &config("https://example.test/v1", "old-model", "dummy-test-key")).unwrap();
    let original = fs::read(&path).unwrap();

    // Refuse all sharing so MoveFileEx cannot replace this still-open destination.
    let exclusive = OpenOptions::new().read(true).share_mode(0).open(&path).unwrap();
    let replacement = config("https://example.test/v1", "new-model", "dummy-test-key");
    let error = store.save(&path, &replacement).unwrap_err();
    assert_rejected_without_key(&error);
    drop(exclusive);

    assert!(fs::read(&path).unwrap() == original, "failed replace changed existing settings");
    let entries: Vec<_> = fs::read_dir(&dir.0).unwrap().map(|entry| entry.unwrap().file_name()).collect();
    assert_eq!(entries.len(), 1, "failed replace left a temporary file");
    assert_eq!(entries[0].as_os_str(), path.file_name().unwrap());
}

#[cfg(windows)]
#[test]
fn symlink_targets_are_rejected_if_symlinks_are_available() {
    let dir = TestDir::new();
    let path = dir.settings_path();
    let target = dir.0.join("target.json");
    fs::write(&target, b"sentinel").unwrap();
    if std::os::windows::fs::symlink_file(&target, &path).is_err() { return; }

    let store = SettingsStore::default();
    let valid = config("https://example.test/v1", "model", "dummy-test-key");
    assert!(store.load(&path).is_err());
    assert!(store.save(&path, &valid).is_err());
    assert!(store.clear(&path).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"sentinel");
}
