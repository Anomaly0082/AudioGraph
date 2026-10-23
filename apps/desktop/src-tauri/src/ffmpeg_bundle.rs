//! Locate the separately replaceable FFmpeg distribution shipped with the app.
//! Hashes are checked while preparing a release, not here: LGPL users may replace
//! the executable and shared libraries with compatible modified versions.
use std::fs;
use std::path::{Path, PathBuf};

fn inspect_bundle(root: &Path) -> Result<PathBuf, String> {
    crate::tool_workspaces::check_existing_chain(root)?;
    let executable = root.join("bin").join(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" });
    crate::tool_workspaces::check_existing_chain(&executable)?;
    let metadata = crate::tool_workspaces::normal_metadata(&executable)?
        .ok_or("应用自带的 FFmpeg 缺失，请重新准备或修复安装包")?;
    if !metadata.is_file() { return Err("应用自带的 FFmpeg 不是普通文件，请修复安装包".into()); }
    // Only presence/type checks, deliberately no version/hash pin at runtime.
    // Loading-compatible replacement LGPL libraries remains supported.
    #[cfg(windows)]
    for prefix in ["avcodec-", "avdevice-", "avfilter-", "avformat-", "avutil-", "swresample-", "swscale-"] {
        let mut found = false;
        for item in fs::read_dir(root.join("bin")).map_err(|error| format!("无法读取 FFmpeg 运行库：{error}"))? {
            let item = item.map_err(|error| format!("无法读取 FFmpeg 运行库：{error}"))?;
            let name = item.file_name().to_string_lossy().to_ascii_lowercase();
            if name.starts_with(prefix) && name.ends_with(".dll")
                && crate::tool_workspaces::normal_metadata(&item.path())?.is_some_and(|meta| meta.is_file()) {
                found = true;
            }
        }
        if !found { return Err(format!("FFmpeg 运行库 {prefix}*.dll 缺失，请修复安装包")); }
    }
    fs::canonicalize(executable).map_err(|error| format!("无法定位应用自带 FFmpeg：{error}"))
}

pub fn locate_bundled() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|error| format!("无法定位应用目录：{error}"))?;
    let directory = executable.parent().ok_or("无法定位应用目录")?;
    let roots = [directory.join("ffmpeg"), directory.join("resources").join("ffmpeg")];
    for root in roots {
        // A present but damaged installation must fail; never silently select
        // another executable from PATH or the process working directory.
        if bundle_entry_present(&root)? { return inspect_bundle(&root); }
    }
    #[cfg(test)]
    {
        // Cargo's test harness lives in target/debug/deps; build.rs copies the
        // same runtime to target/debug/ffmpeg. A copied application must never
        // fall back to an absolute source-checkout path.
        if directory.file_name().is_some_and(|name| name == "deps") {
            if let Some(profile) = directory.parent() {
                let development = profile.join("ffmpeg");
                if bundle_entry_present(&development)? { return inspect_bundle(&development); }
            }
        }
    }
    Err("应用自带的 FFmpeg 未准备好；请运行 scripts/prepare-ffmpeg.ps1 或修复安装包".into())
}

fn bundle_entry_present(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("无法检查 FFmpeg 目录：{error}")),
    }
}

pub fn locate_bundled_probe() -> Result<PathBuf, String> {
    let ffmpeg = locate_bundled()?;
    let probe = ffmpeg.parent().ok_or("无法定位 FFmpeg 运行库目录")?
        .join(if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" });
    crate::tool_workspaces::check_existing_chain(&probe)?;
    if !crate::tool_workspaces::normal_metadata(&probe)?.is_some_and(|meta| meta.is_file()) {
        return Err("应用自带的 ffprobe 缺失，请修复安装包".into());
    }
    fs::canonicalize(probe).map_err(|error| format!("无法定位应用自带 ffprobe：{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    #[test]
    fn missing_bundle_never_resolves_a_path_executable() {
        let root = std::env::temp_dir().join(format!("ffmpeg-bundle-test-{}-{}", std::process::id(), SERIAL.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&root).unwrap();
        assert!(inspect_bundle(&root).is_err());
        fs::remove_dir(root).unwrap();
    }
}
