//! Build-time snapshots. Existing runtimes are retained, including user replacements.
use std::{fs, io, path::Path, sync::atomic::{AtomicU64, Ordering}};
static NEXT: AtomicU64 = AtomicU64::new(0);

fn metadata(path: &Path) -> io::Result<fs::Metadata> {
    let meta = fs::symlink_metadata(path)?;
    let mut linked = meta.file_type().is_symlink();
    #[cfg(windows)] {
        use std::os::windows::fs::MetadataExt;
        linked |= meta.file_attributes() & 0x400 != 0;
    }
    if linked { return Err(io::Error::other("FFmpeg snapshot contains a link or reparse point")); }
    Ok(meta)
}

fn validate(path: &Path) -> io::Result<()> {
    // Inspect ancestors too: a regular-looking child under a junction is unsafe.
    for ancestor in path.ancestors() { if ancestor.as_os_str().is_empty() { continue; } metadata(ancestor)?; }
    let meta = metadata(path)?;
    if meta.is_dir() { for item in fs::read_dir(path)? { validate(&item?.path())?; } }
    else if !meta.is_file() { return Err(io::Error::other("FFmpeg snapshot requires regular files")); }
    Ok(())
}

fn equal(source: &Path, target: &Path) -> io::Result<bool> {
    let a = metadata(source)?; let b = metadata(target)?;
    if a.is_file() && b.is_file() { return Ok(a.len() == b.len() && fs::read(source)? == fs::read(target)?); }
    if !a.is_dir() || !b.is_dir() { return Ok(false); }
    let mut left = fs::read_dir(source)?.map(|e| e.map(|e| e.file_name())).collect::<io::Result<Vec<_>>>()?;
    let mut right = fs::read_dir(target)?.map(|e| e.map(|e| e.file_name())).collect::<io::Result<Vec<_>>>()?;
    left.sort(); right.sort();
    if left != right { return Ok(false); }
    for name in left { if !equal(&source.join(&name), &target.join(name))? { return Ok(false); } }
    Ok(true)
}

fn copy(source: &Path, target: &Path) -> io::Result<()> {
    if metadata(source)?.is_dir() {
        fs::create_dir(target)?;
        for item in fs::read_dir(source)? { let item = item?; copy(&item.path(), &target.join(item.file_name()))?; }
    } else { fs::copy(source, target)?; }
    Ok(())
}

pub fn replace_snapshot(source: &Path, target: &Path) -> io::Result<()> {
    validate(source)?;
    let parent = target.parent().ok_or_else(|| io::Error::other("snapshot target needs parent"))?;
    for ancestor in parent.ancestors() { if !ancestor.as_os_str().is_empty() { metadata(ancestor)?; } }
    let exists = match fs::symlink_metadata(target) { Ok(_) => { validate(target)?; true }, Err(e) if e.kind() == io::ErrorKind::NotFound => false, Err(e) => return Err(e) };
    if exists && equal(source, target)? { return Ok(()); }
    // Unique sibling paths keep rename on one volume and never overwrite a backup.
    let (stage, backup) = loop {
        let id = format!("{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        let stage = parent.join(format!(".ffmpeg-stage-{id}"));
        let backup = parent.join(format!(".ffmpeg-backup-{id}"));
        if !stage.try_exists()? && !backup.try_exists()? { break (stage, backup); }
    };
    copy(source, &stage)?;
    if exists { fs::rename(target, &backup)?; }
    if let Err(error) = fs::rename(&stage, target) {
        if exists { fs::rename(&backup, target).map_err(|rollback| io::Error::other(format!("snapshot install failed: {error}; rollback failed: {rollback}; previous runtime retained at {}", backup.display())))?; }
        return Err(error);
    }
    if exists { println!("cargo:warning=Previous FFmpeg runtime retained at {}", backup.display()); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replaces_entire_snapshot_and_preserves_replacements() {
        let root = std::env::temp_dir().join(format!("ffmpeg-snapshot-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(root.join("source/bin")).unwrap();
        fs::write(root.join("source/bin/avcodec.dll"), b"new").unwrap();
        fs::create_dir_all(root.join("target/bin")).unwrap();
        fs::write(root.join("target/bin/swscale.dll"), b"user replacement").unwrap();
        replace_snapshot(&root.join("source"), &root.join("target")).unwrap();
        assert!(!root.join("target/bin/swscale.dll").exists());
        let backups = || fs::read_dir(&root).unwrap().filter_map(Result::ok).filter(|e| e.file_name().to_string_lossy().starts_with(".ffmpeg-backup-")).map(|e| e.path()).collect::<Vec<_>>();
        assert_eq!(backups().len(), 1);
        assert_eq!(fs::read(backups()[0].join("bin/swscale.dll")).unwrap(), b"user replacement");
        replace_snapshot(&root.join("source"), &root.join("target")).unwrap();
        assert_eq!(backups().len(), 1);
        fs::write(root.join("source/bin/avcodec.dll"), b"NEW").unwrap();
        replace_snapshot(&root.join("source"), &root.join("target")).unwrap();
        assert_eq!(fs::read(root.join("target/bin/avcodec.dll")).unwrap(), b"NEW");
        assert_eq!(backups().len(), 2);
        fs::remove_dir_all(root).unwrap();
    }
}
