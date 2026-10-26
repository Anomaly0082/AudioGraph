mod build_resources;
mod ffmpeg_distribution;
fn main() {
    println!("cargo:rerun-if-changed=resources/ffmpeg");
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
        ffmpeg_distribution::verify(&runtime).expect("FFmpeg 音频精简版源码、运行库或许可材料不完整；release 构建已阻止");
    }
    if runtime.join("bin").join("ffmpeg.exe").is_file() {
        // Cargo tests and --no-bundle builds need the same adjacent resources as
        // the installed/portable application. Never download during a build.
        let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
        if let Some(profile) = out.ancestors().nth(3) {
            build_resources::replace_snapshot(&runtime, &profile.join("ffmpeg")).expect("cannot copy bundled FFmpeg runtime");
        }
    } else {
        println!("cargo:warning=Bundled FFmpeg is missing; run scripts/prepare-ffmpeg.ps1 -RuntimeOnly for local development");
    }
    tauri_build::build()
}

