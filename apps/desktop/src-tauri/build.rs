fn main() {
    println!("cargo:rerun-if-changed=resources/ffmpeg/bundle-provenance.json");
    println!("cargo:rerun-if-env-changed=DEP_TAURI_DEV");
    // Embed the mode in this artifact, rather than a shared target/<profile>
    // side file which another Cargo invocation can overwrite independently.
    let mode = serde_json::json!({
        "schema_version": 1,
        "profile": std::env::var("PROFILE").unwrap_or_default(),
        "frontend_embedded": std::env::var("DEP_TAURI_DEV").as_deref() == Ok("false")
    });
    println!("cargo:rustc-env=AUDIOPROCESS_DESKTOP_BUILD_INFO=AUDIOPROCESS_DESKTOP_BUILD_INFO_V1_BEGIN{}AUDIOPROCESS_DESKTOP_BUILD_INFO_V1_END", mode);
    let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let runtime = manifest.join("resources").join("ffmpeg");
    if std::env::var("PROFILE").as_deref() == Ok("release") {
        let approved = std::fs::read(runtime.join("bundle-provenance.json")).ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some_and(|metadata| metadata.get("redistribution_ready").and_then(serde_json::Value::as_bool) == Some(true));
        assert!(approved, "FFmpeg 匹配依赖源码和许可材料尚未齐备；标准 release/分发构建已阻止。当前仅可使用本地 debug 开发版。");
    }
    if runtime.join("bin").join("ffmpeg.exe").is_file() {
        // Cargo tests and --no-bundle builds need the same adjacent resources as
        // the installed/portable application. Never download during a build.
        let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
        if let Some(profile) = out.ancestors().nth(3) {
            copy_resources(&runtime, &profile.join("ffmpeg")).expect("cannot copy bundled FFmpeg runtime");
        }
    } else {
        println!("cargo:warning=Bundled FFmpeg is missing; run scripts/prepare-ffmpeg.ps1 -RuntimeOnly for local development");
    }
    tauri_build::build()
}

fn copy_resources(source: &std::path::Path, target: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(target)?;
    for item in std::fs::read_dir(source)? {
        let item = item?;
        let metadata = item.metadata()?;
        let destination = target.join(item.file_name());
        if metadata.is_dir() { copy_resources(&item.path(), &destination)?; }
        else if metadata.is_file() {
            let unchanged = destination.metadata().is_ok_and(|existing| existing.len() == metadata.len()
                && existing.modified().ok() >= metadata.modified().ok());
            if !unchanged { std::fs::copy(item.path(), destination)?; }
        }
    }
    Ok(())
}

