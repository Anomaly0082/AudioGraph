//! Build-time validation of the locally prepared FFmpeg source/notice bundle.
//! Deliberately unused by the running app, which permits compatible replacements.
use std::{collections::HashSet, fs, io::Read, path::{Component, Path}};
use sha2::{Digest, Sha256};
const REQUIRED: &[&str] = &["bin/ffmpeg.exe", "bin/ffprobe.exe", "bin/libmp3lame-0.dll", "bin/avcodec-62.dll", "bin/avformat-62.dll", "bin/avfilter-11.dll", "bin/avutil-60.dll", "bin/swresample-6.dll",
    "NOTICE.txt", "sources/ffmpeg-audio-sources.zip", "licenses/FFmpeg/COPYING.LGPLv2.1", "licenses/FFmpeg/COPYING.GPLv2", "licenses/FFmpeg/LICENSE.md", "licenses/FFmpeg/CREDITS",
    "licenses/LAME/COPYING", "licenses/LAME/LICENSE", "licenses/GCC/COPYING.RUNTIME", "licenses/GCC/COPYING3", "licenses/GCC/COPYING.LIB", "licenses/GCC/README",
    "licenses/MinGW/COPYING", "licenses/MinGW/COPYING.MinGW-w64-runtime.txt", "licenses/MinGW/COPYING.MinGW-w64.txt", "licenses/winpthreads/COPYING"];

fn listed_tree(root: &Path, dir: &Path, expected: &HashSet<String>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        #[cfg(windows)] { use std::os::windows::fs::MetadataExt; if meta.file_attributes() & 0x400 != 0 { return Err("Linked FFmpeg material".into()); } }
        if meta.file_type().is_symlink() { return Err("Linked FFmpeg material".into()); }
        if meta.is_dir() { listed_tree(root, &path, expected)?; }
        else {
            let relative = path.strip_prefix(root).map_err(|e| e.to_string())?.to_string_lossy().replace('\\', "/");
            if !meta.is_file() || (relative != "bundle-provenance.json" && !expected.contains(&relative.to_lowercase())) {
                return Err(format!("Unlisted FFmpeg distribution file: {relative}"));
            }
        }
    }
    Ok(())
}

pub fn verify(root: &Path) -> Result<(), String> {
    let bytes = fs::read(root.join("bundle-provenance.json")).map_err(|e| e.to_string())?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if value["schema"] != 2 || value["provider"] != "AudioGraph audio-only build" || value["redistribution_ready"] != true {
        return Err("Audio-only FFmpeg source and notice materials are not ready".into());
    }
    let entries = value["distribution_files"].as_array().ok_or("Missing distribution file manifest")?;
    let mut expected = HashSet::new();
    for entry in entries {
        let relative = entry["path"].as_str().ok_or("Invalid file path")?;
        if relative.is_empty() || relative.contains(['\\', ':']) || Path::new(relative).components().any(|c| !matches!(c, Component::Normal(_)))
            || !expected.insert(relative.to_lowercase()) {
            return Err("Invalid or duplicate FFmpeg material path".into());
        }
        let path = root.join(relative);
        for ancestor in path.ancestors() {
            let meta = fs::symlink_metadata(ancestor).map_err(|e| e.to_string())?;
            #[cfg(windows)] { use std::os::windows::fs::MetadataExt; if meta.file_attributes() & 0x400 != 0 { return Err("Linked FFmpeg material".into()); } }
            if meta.file_type().is_symlink() { return Err("Linked FFmpeg material".into()); }
        }
        let mut file = fs::File::open(&path).map_err(|e| e.to_string())?;
        let meta = file.metadata().map_err(|e| e.to_string())?;
        if !meta.is_file() || entry["bytes"].as_u64() != Some(meta.len()) { return Err(format!("FFmpeg material size mismatch: {relative}")); }
        let mut hash = Sha256::new();
        let mut buffer = [0; 64 * 1024];
        loop { let count = file.read(&mut buffer).map_err(|e| e.to_string())?; if count == 0 { break; } hash.update(&buffer[..count]); }
        if entry["sha256"].as_str() != Some(format!("{:x}", hash.finalize()).as_str()) { return Err(format!("FFmpeg material hash mismatch: {relative}")); }
    }
    for required in REQUIRED {
        if !expected.contains(&required.to_lowercase()) { return Err(format!("Missing FFmpeg source/license entry: {required}")); }
    }
    listed_tree(root, root, &expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_gate_requires_real_sources_and_rejects_changed_notices() {
        let root = std::env::temp_dir().join(format!("ffmpeg-distribution-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let names = REQUIRED;
        let entries: Vec<_> = names.iter().map(|name| {
            let path = root.join(name); fs::create_dir_all(path.parent().unwrap()).unwrap(); fs::write(&path, b"fixture").unwrap();
            serde_json::json!({"path":name,"bytes":7,"sha256":format!("{:x}",Sha256::digest(b"fixture"))})
        }).collect();
        let record = serde_json::json!({"schema":2,"provider":"AudioGraph audio-only build","redistribution_ready":true,"distribution_files":entries});
        fs::write(root.join("bundle-provenance.json"), record.to_string()).unwrap();
        assert!(verify(&root).is_ok());
        fs::write(root.join("bin/unlisted.dll"), b"extra").unwrap();
        assert!(verify(&root).unwrap_err().contains("Unlisted"));
        fs::remove_file(root.join("bin/unlisted.dll")).unwrap();
        fs::write(root.join("licenses/LAME/COPYING"), b"changed").unwrap();
        assert!(verify(&root).unwrap_err().contains("hash mismatch"));
        fs::write(root.join("licenses/LAME/COPYING"), b"fixture").unwrap();
        fs::remove_file(root.join("sources/ffmpeg-audio-sources.zip")).unwrap();
        assert!(verify(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
