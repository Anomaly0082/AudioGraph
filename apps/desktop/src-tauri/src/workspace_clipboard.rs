//! Manual path copying only; never exposes clipboard read or arbitrary text to AI.
use crate::{backend::BackendManager, tool_workspaces::ToolWorkspaces};
use std::{path::Path, sync::Arc};
use tauri::Manager;

fn display_path(path: &Path) -> Result<String, String> {
    let text = path.to_str().ok_or("路径不是有效文本")?;
    if let Some(tail) = text.strip_prefix(r"\\?\UNC\") {
        return Ok(format!(r"\\{tail}"));
    }
    if let Some(tail) = text.strip_prefix(r"\\?\") {
        let bytes = tail.as_bytes();
        if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\' {
            return Ok(tail.into());
        }
        return Err("不支持复制此设备路径".into());
    }
    Ok(text.into())
}

fn path_text(spaces: &ToolWorkspaces, space: &str, path: &str, relative: bool) -> Result<String, String> {
    if path.is_empty() { return Err("请先选择文件或文件夹".into()); }
    let target = spaces.checked_path(space, path, false)?;
    let metadata = std::fs::metadata(&target).map_err(|_| "所选条目已不存在，请刷新")?;
    if !metadata.is_file() && !metadata.is_dir() { return Err("不支持此文件类型".into()); }
    if relative { Ok(path.into()) } else { display_path(&target) }
}

fn clipboard_units(text: &str) -> Result<Vec<u16>, String> {
    if text.is_empty() || text.contains('\0') { return Err("路径文本无效".into()); }
    let mut units: Vec<u16> = text.encode_utf16().collect();
    if units.len() > 32767 { return Err("路径过长，无法复制".into()); }
    units.push(0);
    Ok(units)
}

#[cfg(windows)]
fn write_clipboard(owner: usize, text: &str) -> Result<(), String> {
    use windows::Win32::{Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND}, System::{
        DataExchange::{OpenClipboard, CloseClipboard, EmptyClipboard, SetClipboardData},
        Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE, GMEM_ZEROINIT},
    }};
    struct Memory(HGLOBAL);
    impl Drop for Memory { fn drop(&mut self) { unsafe { let _ = GlobalFree(Some(self.0)); } } }
    struct Clipboard;
    impl Drop for Clipboard { fn drop(&mut self) { unsafe { let _ = CloseClipboard(); } } }
    let units = clipboard_units(text)?;
    if owner == 0 { return Err("复制路径的窗口已关闭".into()); }
    // Prepare the owned allocation before touching the user's clipboard.
    let memory = Memory(unsafe { GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, units.len() * 2) }.map_err(|_| "无法准备剪贴板内容")?);
    let buffer = unsafe { GlobalLock(memory.0) };
    if buffer.is_null() { return Err("无法准备剪贴板内容".into()); }
    unsafe {
        std::ptr::copy_nonoverlapping(units.as_ptr(), buffer.cast::<u16>(), units.len());
        let _ = GlobalUnlock(memory.0);
    }
    // Use the real invoking window, not NULL: EmptyClipboard would otherwise
    // leave no owner and SetClipboardData could fail. Short retries handle a
    // transient clipboard lock without holding the UI thread.
    let owner = HWND(owner as *mut std::ffi::c_void);
    let mut opened = false;
    for attempt in 0..5 {
        if unsafe { OpenClipboard(Some(owner)) }.is_ok() { opened = true; break; }
        if attempt != 4 { std::thread::sleep(std::time::Duration::from_millis(20)); }
    }
    if !opened { return Err("剪贴板正被占用，请重试复制路径".into()); }
    let _clipboard = Clipboard;
    unsafe { EmptyClipboard() }.map_err(|_| "无法更新剪贴板")?;
    // CF_UNICODETEXT = 13. Windows takes ownership only after a successful set.
    unsafe { SetClipboardData(13, Some(HANDLE(memory.0.0))) }.map_err(|_| "复制路径失败，请重试")?;
    std::mem::forget(memory);
    Ok(())
}

#[tauri::command]
pub async fn workspace_copy_path(app: tauri::AppHandle, window: tauri::Window,
    backend: tauri::State<'_, Arc<BackendManager>>, session_id: String, space: String,
    path: String, relative: Option<bool>) -> Result<(), String> {
    #[cfg(windows)] let owner = window.hwnd().map_err(|_| "无法访问当前窗口")?.0 as usize;
    #[cfg(not(windows))] { let _ = window; }
    let data = app.path().app_data_dir().map_err(|_| "无法定位应用数据目录")?;
    let manager = backend.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.with_workspace_operation(&session_id, |root| {
        let spaces = ToolWorkspaces::new_read_only(root, &data)?;
        let text = path_text(&spaces, &space, &path, relative.unwrap_or(false))?;
        #[cfg(windows)] { write_clipboard(owner, &text) }
        #[cfg(not(windows))] { let _ = text; Err("此平台暂不支持原生路径复制".into()) }
    })).await.map_err(|_| "复制路径任务失败".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn display_paths_remove_only_known_windows_prefixes_and_encode_unicode() {
        assert_eq!(display_path(Path::new(r"\\?\C:\音频 空格\clip.wav")).unwrap(), r"C:\音频 空格\clip.wav");
        assert_eq!(display_path(Path::new(r"\\?\UNC\server\share\音频.wav")).unwrap(), r"\\server\share\音频.wav");
        assert!(display_path(Path::new(r"\\?\GLOBALROOT\Device\secret")).is_err());
        assert_eq!(String::from_utf16(&clipboard_units("中文🎵").unwrap()[..4]).unwrap(), "中文🎵");
        assert_eq!(clipboard_units("x").unwrap(), vec![120,0]);
        assert!(clipboard_units("a\0b").is_err());
        assert!(clipboard_units(&"x".repeat(32768)).is_err());
    }
    #[test]
    fn copy_text_is_scoped_to_existing_user_or_ai_entries_without_writing_clipboard() {
        let root = std::env::temp_dir().join(format!("copy-path-{}-{}", std::process::id(), crate::run_records::time_ms().unwrap()));
        std::fs::create_dir_all(root.join("user")).unwrap();
        let spaces = ToolWorkspaces::new(&root.join("user"), &root.join("data")).unwrap();
        std::fs::create_dir_all(spaces.ai_root.join("音频")).unwrap();
        std::fs::write(spaces.user_root.join("clip.wav"), b"original").unwrap();
        std::fs::write(spaces.ai_root.join("音频/out.wav"), b"result").unwrap();
        assert_eq!(path_text(&spaces,"user","clip.wav",true).unwrap(), "clip.wav");
        assert_eq!(path_text(&spaces,"ai","音频/out.wav",true).unwrap(), "音频/out.wav");
        assert_eq!(path_text(&spaces,"ai","音频/out.wav",false).unwrap(), display_path(&spaces.ai_root.join("音频/out.wav")).unwrap());
        assert!(path_text(&spaces,"ai","音频",false).is_ok());
        for path in ["", "../outside.wav", "C:/outside.wav", "missing.wav"] {
            assert!(path_text(&spaces,"user",path,false).is_err());
        }
        assert!(path_text(&spaces,"other","clip.wav",false).is_err());
        std::fs::hard_link(spaces.user_root.join("clip.wav"), spaces.user_root.join("linked.wav")).unwrap();
        assert!(path_text(&spaces,"user","linked.wav",false).is_err());
        assert_eq!(std::fs::read(spaces.user_root.join("clip.wav")).unwrap(), b"original");
        std::fs::remove_dir_all(root).unwrap();
    }
}
