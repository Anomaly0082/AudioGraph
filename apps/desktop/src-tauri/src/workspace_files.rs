//! Manual file operations. Callers must hold the session/agent idle lease.
use crate::tool_workspaces::{ToolWorkspaces, check_existing_chain, is_hardlinked, normal_metadata};
use serde_json::{Value,json};
use std::fs::{self,File,OpenOptions};
use std::io::Read;
use std::path::{Path,PathBuf};
use std::sync::atomic::{AtomicU64,Ordering};

const BYTE_LIMIT:u64=1024*1024*1024;
const ENTRY_LIMIT:usize=10_000;
const DEPTH_LIMIT:usize=16;
static NEXT:AtomicU64=AtomicU64::new(0);

fn err(e:std::io::Error)->String { format!("文件操作失败：{e}") }
fn target(spaces:&ToolWorkspaces,space:&str,path:&str,missing:bool)->Result<PathBuf,String>{
    if path.is_empty(){return Err("不能修改工作区根目录".into());}
    spaces.checked_path(space,path,missing)
}
fn vacant(path:&Path)->Result<(),String>{
    check_existing_chain(path)?;
    if normal_metadata(path)?.is_some(){return Err("目标已存在，请使用其他名称".into());}
    let parent=path.parent().ok_or("目标没有父目录")?;
    if !normal_metadata(parent)?.is_some_and(|m|m.is_dir()){return Err("目标文件夹不存在".into());}
    Ok(())
}

pub(crate) fn mkdir(spaces:&ToolWorkspaces,space:&str,path:&str)->Result<Value,String>{
    let pathbuf=target(spaces,space,path,true)?;
    vacant(&pathbuf)?;
    fs::create_dir(&pathbuf).map_err(err)?;
    Ok(json!({"space":space,"path":path}))
}

#[derive(Clone,PartialEq,Eq)]
struct Item { relative:PathBuf, directory:bool, bytes:u64, modified:Option<std::time::SystemTime> }
fn inspect_tree(source:&Path)->Result<Vec<Item>,String>{
    fn walk(source:&Path,relative:PathBuf,depth:usize,items:&mut Vec<Item>,bytes:&mut u64)->Result<(),String>{
        if depth>DEPTH_LIMIT || items.len()>=ENTRY_LIMIT{return Err("操作超过 10000 个条目或 16 层目录限制".into());}
        check_existing_chain(source)?;
        let meta=normal_metadata(source)?.ok_or("源文件已不存在")?;
        if !meta.is_dir() && (!meta.is_file() || is_hardlinked(source,&meta)){return Err("仅支持普通文件和文件夹，不支持链接".into());}
        if meta.is_file(){*bytes=bytes.checked_add(meta.len()).ok_or("文件总大小超限")?;}
        if *bytes>BYTE_LIMIT{return Err("操作超过 1 GiB 大小限制".into());}
        items.push(Item{relative:relative.clone(),directory:meta.is_dir(),bytes:meta.len(),modified:meta.modified().ok()});
        if meta.is_dir(){
            let mut entries=Vec::new();
            for entry in fs::read_dir(source).map_err(err)?{
                if entries.len()+items.len()>=ENTRY_LIMIT{return Err("操作超过 10000 个条目限制".into());}
                entries.push(entry.map_err(err)?);
            }
            entries.sort_by_key(|entry|entry.file_name());
            for entry in entries{
                let name=entry.file_name();
                let name=name.to_str().ok_or("不支持此文件名")?;
                // Apply exactly the workspace component policy, including ADS/device names.
                let dummy=ToolWorkspaces{user_root:source.to_path_buf(),ai_root:source.to_path_buf()};
                dummy.checked_path("user",name,false)?;
                walk(&entry.path(),relative.join(name),depth+1,items,bytes)?;
            }
        }
        Ok(())
    }
    let mut items=Vec::new();walk(source,PathBuf::new(),0,&mut items,&mut 0)?;Ok(items)
}

#[cfg(windows)]
fn publish(source:&Path,target:&Path)->Result<(),String>{
    use std::os::windows::ffi::OsStrExt;
    #[link(name="Kernel32")]
    unsafe extern "system" {fn MoveFileExW(from:*const u16,to:*const u16,flags:u32)->i32;}
    let from:Vec<u16>=source.as_os_str().encode_wide().chain(Some(0)).collect();
    let to:Vec<u16>=target.as_os_str().encode_wide().chain(Some(0)).collect();
    // No REPLACE_EXISTING and no cross-volume COPY_ALLOWED fallback.
    if unsafe{MoveFileExW(from.as_ptr(),to.as_ptr(),8)}==0{return Err(err(std::io::Error::last_os_error()));}Ok(())
}
#[cfg(not(windows))]
fn publish(_source:&Path,_target:&Path)->Result<(),String>{Err("此文件操作仅支持 Windows".into())}

fn stage_copy(source:&Path,destination:&Path,items:&[Item])->Result<(),String>{
    for item in items{
        let from=if item.relative.as_os_str().is_empty(){source.to_path_buf()}else{source.join(&item.relative)};
        let to=if item.relative.as_os_str().is_empty(){destination.to_path_buf()}else{destination.join(&item.relative)};
        check_existing_chain(&from)?;
        let before=normal_metadata(&from)?.ok_or("源文件已不存在")?;
        if before.is_dir()!=item.directory || before.len()!=item.bytes || before.modified().ok()!=item.modified || is_hardlinked(&from,&before){return Err("操作期间源文件已变化，请刷新重试".into());}
        if item.directory{fs::create_dir(&to).map_err(err)?;}else{
            let mut input=File::open(&from).map_err(err)?;
            let opened=input.metadata().map_err(err)?;
            if opened.len()!=item.bytes || opened.modified().ok()!=item.modified
                || !crate::run_records::opened_file_within_workspace(&input,from.parent().ok_or("源文件没有父目录")?){return Err("源文件已变化或越界，请刷新重试".into());}
            let mut output=OpenOptions::new().write(true).create_new(true).open(&to).map_err(err)?;
            let copied=std::io::copy(&mut Read::by_ref(&mut input).take(item.bytes+1),&mut output).map_err(err)?;
            output.sync_all().map_err(err)?;
            let after=input.metadata().map_err(err)?;
            if copied!=item.bytes || after.len()!=item.bytes || after.modified().ok()!=item.modified
                || !crate::run_records::opened_file_within_workspace(&input,from.parent().ok_or("源文件没有父目录")?){return Err("复制期间源文件已变化，请刷新重试".into());}
            check_existing_chain(&from)?;
        }
    }Ok(())
}

fn copy_one(source:&Path,destination:&Path)->Result<(),String>{
    let items=inspect_tree(source)?;vacant(destination)?;
    let parent=destination.parent().ok_or("目标没有父目录")?;
    // Reserve a unique directory first; all temporary children belong to this operation.
    let stage=loop{
        let path=parent.join(format!(".manual-tmp-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
        match fs::create_dir(&path){Ok(())=>break path,Err(e) if e.kind()==std::io::ErrorKind::AlreadyExists=>continue,Err(e)=>return Err(err(e))}
    };
    let payload=stage.join("payload");
    let result=(||{stage_copy(source,&payload,&items)?;
        if inspect_tree(source)?!=items{return Err("复制期间源目录已变化，请刷新重试".into());}
        vacant(destination)?;publish(&payload,destination)})();
    let _=fs::remove_dir_all(&stage); // Only this operation's newly reserved staging directory.
    result
}

pub(crate) fn transfer(spaces:&ToolWorkspaces,space:&str,source:&str,destination:&str,move_entry:bool)->Result<Value,String>{
    let from=target(spaces,space,source,false)?;let to=target(spaces,space,destination,true)?;
    let key=|path:&Path|path.components().map(|p|p.as_os_str().to_string_lossy().to_lowercase()).collect::<Vec<_>>();
    // Copy/paste in the same folder creates a sibling, never replaces the original.
    if !move_entry && key(&to) == key(&from) {
        let (parent, name) = source.rsplit_once('/').unwrap_or(("", source));
        let is_directory = normal_metadata(&from)?.ok_or("源文件已不存在")?.is_dir();
        let file_name = Path::new(name);
        let extension = if is_directory { None } else { file_name.extension().and_then(|v| v.to_str()) };
        let stem = if extension.is_some() { file_name.file_stem().and_then(|v| v.to_str()).unwrap_or(name) } else { name };
        for index in 1..=1000 {
            let suffix = if index == 1 { " 副本".into() } else { format!(" 副本 ({index})") };
            let sibling = format!("{stem}{suffix}{}", extension.map(|ext| format!(".{ext}")).unwrap_or_default());
            let candidate = if parent.is_empty() { sibling } else { format!("{parent}/{sibling}") };
            let path = target(spaces, space, &candidate, true)?;
            if normal_metadata(&path)?.is_some() { continue; }
            copy_one(&from, &path)?;
            return Ok(json!({"space":space,"path":candidate,"source_path":source,"moved":false}));
        }
        return Err("同名副本过多，请复制到其他文件夹".into());
    }
    if key(&to).starts_with(&key(&from)){return Err("目标不能是源路径或其子目录".into());}
    inspect_tree(&from)?;vacant(&to)?;
    if move_entry{publish(&from,&to)?;}else{copy_one(&from,&to)?;}
    Ok(json!({"space":space,"path":destination,"source_path":source,"moved":move_entry}))
}

/// Only native drop event paths may enter here. Never expose this absolute-path API as a command.
pub(crate) fn import_files(spaces:&ToolWorkspaces,space:&str,directory:&str,sources:&[PathBuf])->Result<Value,String>{
    if sources.is_empty() || sources.len()>32{return Err("一次请拖入 1 至 32 个文件".into());}
    let folder=spaces.checked_path(space,directory,false)?;
    if !normal_metadata(&folder)?.is_some_and(|m|m.is_dir()){return Err("请选择导入目标文件夹".into());}
    let mut plans=Vec::new();let mut bytes=0u64;
    for source in sources{
        if !source.is_absolute() || source.components().any(|p|matches!(p,std::path::Component::ParentDir|std::path::Component::CurDir)){return Err("拖入文件路径无效，请重新拖入".into());}
        let items=inspect_tree(source)?;
        if items.len()!=1 || items[0].directory{return Err("暂不支持导入文件夹，请只拖入文件".into());}
        bytes=bytes.checked_add(items[0].bytes).ok_or("导入文件总大小超限")?;
        if bytes>BYTE_LIMIT{return Err("导入文件总大小超过 1 GiB".into());}
        let name=source.file_name().and_then(|n|n.to_str()).ok_or("不支持此文件名")?;
        let relative=if directory.is_empty(){name.to_owned()}else{format!("{directory}/{name}")};
        let destination=target(spaces,space,&relative,true)?;vacant(&destination)?;
        if plans.iter().any(|(_,p):&(PathBuf,PathBuf)|p.to_string_lossy().to_lowercase()==destination.to_string_lossy().to_lowercase()){return Err("拖入的文件存在重名，请分别导入".into());}
        plans.push((source.clone(),destination));
    }
    // Every source/destination is validated before the first copy. Individual files publish atomically.
    let mut imported=Vec::new();
    for (source,destination) in plans{
        if let Err(error)=copy_one(&source,&destination){
            return Err(format!("导入已停止：{error}。已导入：{}。原始文件均已保留。",if imported.is_empty(){"无".into()}else{imported.join("、")}));
        }
        imported.push(destination.file_name().unwrap().to_string_lossy().into_owned());
    }
    Ok(json!({"space":space,"path":directory,"imported":imported}))
}

/// Shell parsing names are DOS/UNC paths, not Rust's canonical Win32 namespace paths.
/// Do not blindly strip a prefix: device/volume namespaces must never become shell names.
#[cfg(windows)]
fn shell_file_path(path: &Path) -> Result<PathBuf, String> {
    use std::ffi::OsString;
    use std::path::{Component, Prefix};
    if !path.is_absolute() { return Err("回收站需要完整的本地文件路径".into()); }
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return Err("回收站文件路径无效".into());
    };
    let mut result = match prefix.kind() {
        Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => PathBuf::from(format!("{}:\\", letter as char)),
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
            let mut root = OsString::from(r"\\");
            root.push(server);
            root.push(r"\");
            root.push(share);
            root.push(r"\");
            PathBuf::from(root)
        }
        _ => return Err("此路径类型不支持系统回收站".into()),
    };
    for part in components {
        match part {
            Component::RootDir => {},
            Component::Normal(name) => result.push(name),
            _ => return Err("回收站文件路径未规范化".into()),
        }
    }
    Ok(result)
}

#[cfg(windows)]
pub(crate) fn recycle(spaces:&ToolWorkspaces,space:&str,path:&str)->Result<Value,String>{
    use windows::core::PCWSTR;
    use windows::Win32::System::Com::{CoInitializeEx,CoUninitialize,CoCreateInstance,COINIT_APARTMENTTHREADED,CLSCTX_INPROC_SERVER,IBindCtx};
    use windows::Win32::UI::Shell::{FileOperation,IFileOperation,IShellItem,IFileOperationProgressSink,SHCreateItemFromParsingName,FOFX_RECYCLEONDELETE,FOFX_EARLYFAILURE,FOF_NOCONFIRMATION,FOF_NOERRORUI,FOF_SILENT};
    use std::os::windows::ffi::OsStrExt;
    let target=target(spaces,space,path,false)?;inspect_tree(&target)?;
    let shell_path = shell_file_path(&target)?;
    check_existing_chain(&shell_path)?;
    // DOS normalization (e.g. trailing spaces) must not redirect the operation.
    if fs::canonicalize(&shell_path).map_err(err)? != fs::canonicalize(&target).map_err(err)? {
        return Err("回收站路径转换指向了不同文件，已取消操作".into());
    }
    let name:Vec<u16>=shell_path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe{
        CoInitializeEx(None,COINIT_APARTMENTTHREADED).ok().map_err(|e|format!("无法初始化系统回收站：{e}"))?;
        struct Com;impl Drop for Com{fn drop(&mut self){unsafe{CoUninitialize()}}}let _com=Com;
        let operation:IFileOperation=CoCreateInstance(&FileOperation,None,CLSCTX_INPROC_SERVER).map_err(|e|format!("系统回收站不可用：{e}"))?;
        operation.SetOperationFlags(FOFX_RECYCLEONDELETE|FOFX_EARLYFAILURE|FOF_NOCONFIRMATION|FOF_NOERRORUI|FOF_SILENT).map_err(|e|format!("无法设置回收站操作：{e}"))?;
        let item:IShellItem=SHCreateItemFromParsingName(PCWSTR(name.as_ptr()),None::<&IBindCtx>).map_err(|e|format!("无法读取待回收项目：{e}"))?;
        let sink:IFileOperationProgressSink=RecycleOnly.into();
        operation.DeleteItem(&item,&sink).map_err(|e|format!("无法提交回收站操作：{e}"))?;
        operation.PerformOperations().map_err(|e|format!("移入回收站失败：{e}"))?;
        if operation.GetAnyOperationsAborted().map_err(|e|format!("无法确认回收站结果：{e}"))?.as_bool(){return Err("移入回收站已取消".into());}
    }
    if normal_metadata(&target)?.is_some(){return Err("文件仍在原处，未能移入回收站，请刷新重试".into());}
    Ok(json!({"space":space,"path":path,"recycled":true}))
}
#[cfg(not(windows))]
pub(crate) fn recycle(_spaces:&ToolWorkspaces,_space:&str,_path:&str)->Result<Value,String>{Err("系统回收站仅支持 Windows".into())}

#[cfg(test)]
#[path="workspace_files_tests.rs"]
mod tests;
#[cfg(test)]
#[path="workspace_files_review_tests.rs"]
mod review_tests;

#[cfg(windows)]
use windows::{core::{Ref,PCWSTR,HRESULT,Result as WinResult},Win32::UI::Shell::{IShellItem,IFileOperationProgressSink,IFileOperationProgressSink_Impl}};
#[cfg(windows)]
#[windows::core::implement(IFileOperationProgressSink)]
struct RecycleOnly;
#[cfg(windows)]
#[allow(non_snake_case,unused_variables)]
impl IFileOperationProgressSink_Impl for RecycleOnly_Impl{
    fn StartOperations(&self)->WinResult<()>{Ok(())}
    fn FinishOperations(&self,hrresult:HRESULT)->WinResult<()>{hrresult.ok()}
    fn PreRenameItem(&self,dwflags:u32,psiitem:Ref<'_,IShellItem>,psznewname:&PCWSTR)->WinResult<()>{Ok(())}
    fn PostRenameItem(&self,dwflags:u32,psiitem:Ref<'_,IShellItem>,psznewname:&PCWSTR,hrrename:HRESULT,psinewlycreated:Ref<'_,IShellItem>)->WinResult<()>{hrrename.ok()}
    fn PreMoveItem(&self,dwflags:u32,psiitem:Ref<'_,IShellItem>,psidestinationfolder:Ref<'_,IShellItem>,psznewname:&PCWSTR)->WinResult<()>{Ok(())}
    fn PostMoveItem(&self,dwflags:u32,psiitem:Ref<'_,IShellItem>,psidestinationfolder:Ref<'_,IShellItem>,psznewname:&PCWSTR,hrmove:HRESULT,psinewlycreated:Ref<'_,IShellItem>)->WinResult<()>{hrmove.ok()}
    fn PreCopyItem(&self,dwflags:u32,psiitem:Ref<'_,IShellItem>,psidestinationfolder:Ref<'_,IShellItem>,psznewname:&PCWSTR)->WinResult<()>{Ok(())}
    fn PostCopyItem(&self,dwflags:u32,psiitem:Ref<'_,IShellItem>,psidestinationfolder:Ref<'_,IShellItem>,psznewname:&PCWSTR,hrcopy:HRESULT,psinewlycreated:Ref<'_,IShellItem>)->WinResult<()>{hrcopy.ok()}
    fn PreDeleteItem(&self,dwflags:u32,psiitem:Ref<'_,IShellItem>)->WinResult<()>{
        // TSF_DELETE_RECYCLE_IF_POSSIBLE is supplied for the actual planned action.
        // An error here cancels this and all subsequent operations, before deletion.
        if dwflags & 0x80 == 0 {return Err(windows::core::Error::new(HRESULT(0x80004005u32 as i32),"系统回收站不可用，已阻止永久删除"));}Ok(())
    }
    fn PostDeleteItem(&self,dwflags:u32,psiitem:Ref<'_,IShellItem>,hrdelete:HRESULT,psinewlycreated:Ref<'_,IShellItem>)->WinResult<()>{hrdelete.ok()}
    fn PreNewItem(&self,dwflags:u32,psidestinationfolder:Ref<'_,IShellItem>,psznewname:&PCWSTR)->WinResult<()>{Ok(())}
    fn PostNewItem(&self,dwflags:u32,psidestinationfolder:Ref<'_,IShellItem>,psznewname:&PCWSTR,psztemplatename:&PCWSTR,dwfileattributes:u32,hrnew:HRESULT,psinewitem:Ref<'_,IShellItem>)->WinResult<()>{hrnew.ok()}
    fn UpdateProgress(&self,iworktotal:u32,iworksofar:u32)->WinResult<()>{Ok(())}
    fn ResetTimer(&self)->WinResult<()>{Ok(())}
    fn PauseTimer(&self)->WinResult<()>{Ok(())}
    fn ResumeTimer(&self)->WinResult<()>{Ok(())}
}
