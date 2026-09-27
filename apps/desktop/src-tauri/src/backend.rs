use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) const MAX_REQUEST: usize = 4 * 1024 * 1024;
const MAX_RESPONSE: usize = 8 * 1024 * 1024;
const MAX_PENDING: usize = 8;
const RPC_TIMEOUT: Duration = Duration::from_secs(15);
const EXIT_GRACE: Duration = Duration::from_secs(5);
type Reply = Result<Value, String>;
pub(crate) type DisconnectCallback = Arc<dyn Fn(DisconnectedEvent) + Send + Sync>;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisconnectedEvent {
    pub session_id: String,
    pub message: String,
    pub forced: bool,
}

#[derive(Clone, Serialize)]
pub struct DisconnectReport {
    pub forced: bool,
    pub message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    pub session_id: String,
    pub workspace: String,
    pub allow_devices: bool,
    pub allow_monitor: bool,
    pub capabilities: Value,
    pub previous_forced_disconnect: bool,
}

struct Shared {
    session_id: String,
    alive: AtomicBool,
    pending: Mutex<HashMap<String, mpsc::Sender<Reply>>>,
    writer: Mutex<Option<mpsc::SyncSender<Vec<u8>>>>,
    reason: Mutex<String>,
    stderr: Mutex<VecDeque<u8>>,
    exited: Mutex<Option<DisconnectReport>>,
    exit_ready: Condvar,
    on_disconnect: DisconnectCallback,
}

impl Shared {
    fn close(&self, reason: &str) {
        if !self.alive.swap(false, Ordering::AcqRel) { return; }
        *self.reason.lock().unwrap() = reason.to_owned();
        // 丢弃唯一发送端后，writer放弃尚未写出的请求并关闭stdin，向C++发送EOF。
        self.writer.lock().unwrap().take();
        let pending = std::mem::take(&mut *self.pending.lock().unwrap());
        for (_, response) in pending { let _ = response.send(Err(reason.to_owned())); }
        (self.on_disconnect)(DisconnectedEvent {
            session_id: self.session_id.clone(), message: reason.to_owned(), forced: false,
        });
    }

    fn finish(&self, forced: bool, exit: &str) {
        let reason = self.reason.lock().unwrap().clone();
        let tail: Vec<u8> = self.stderr.lock().unwrap().iter().copied().collect();
        let stderr = String::from_utf8_lossy(&tail);
        let message = if forced {
            format!("{reason} 协作停止超过5秒，仅强制终止本连接拥有的后台进程；任务可能中断并留下部分文件。")
        } else if stderr.trim().is_empty() {
            format!("{reason} 后台进程已退出（{exit}）。")
        } else {
            format!("{reason} 后台进程已退出（{exit}）。{}", stderr.trim())
        };
        *self.exited.lock().unwrap() = Some(DisconnectReport { forced, message: message.clone() });
        self.exit_ready.notify_all();
        if forced {
            (self.on_disconnect)(DisconnectedEvent { session_id: self.session_id.clone(), message, forced });
        }
    }
}

pub(crate) struct Connection {
    shared: Arc<Shared>,
    workspace: PathBuf,
    next_id: AtomicU64,
    threads: Mutex<Vec<thread::JoinHandle<()>>>,
}

impl Connection {
    // executable仅由固定sidecar定位器提供；不暴露给Webview或JSON请求。
    pub(crate) fn spawn(executable: &Path, workspace: PathBuf, session_id: String,
                       allow_devices: bool, allow_monitor: bool,
                       on_disconnect: DisconnectCallback) -> Result<Arc<Self>, String> {
        let mut command = Command::new(executable);
        command.arg("--workspace").arg(&workspace);
        if allow_devices { command.arg("--allow-devices"); }
        if allow_monitor { command.arg("--allow-monitor"); }
        command.current_dir(&workspace).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)] {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW，不弹出额外终端。
        }
        let mut child = command.spawn().map_err(|e| format!("无法启动固定 control-cli：{e}"))?;
        let mut input = child.stdin.take().ok_or("后台进程未提供stdin")?;
        let mut output = child.stdout.take().ok_or("后台进程未提供stdout")?;
        let mut errors = child.stderr.take().ok_or("后台进程未提供stderr")?;
        let (sender, receiver) = mpsc::sync_channel::<Vec<u8>>(MAX_PENDING);
        let shared = Arc::new(Shared {
            session_id, alive: AtomicBool::new(true), pending: Mutex::new(HashMap::new()),
            writer: Mutex::new(Some(sender)), reason: Mutex::new(String::new()),
            stderr: Mutex::new(VecDeque::new()), exited: Mutex::new(None), exit_ready: Condvar::new(), on_disconnect,
        });
        let writer_shared = shared.clone();
        let writer = thread::spawn(move || {
            while let Ok(bytes) = receiver.recv() {
                if !writer_shared.alive.load(Ordering::Acquire) { break; }
                if let Err(error) = input.write_all(&bytes).and_then(|_| input.flush()) {
                    writer_shared.close(&format!("后台请求写入失败：{error}；任务状态可能不确定，请勿自动重试。"));
                    break;
                }
            }
            // drop(input)触发EOF；若write被阻塞，monitor超时终止child后写入会失败。
        });
        let reader_shared = shared.clone();
        let reader = thread::spawn(move || {
            let mut line = Vec::new();
            let mut buffer = [0_u8; 8192];
            loop {
                let count = match output.read(&mut buffer) {
                    Ok(0) => {
                        reader_shared.close("后台连接已关闭；未完成请求的执行结果不确定，请勿自动重试。");
                        break;
                    }
                    Ok(count) => count,
                    Err(error) => { reader_shared.close(&format!("读取后台响应失败：{error}")); break; }
                };
                let mut failed = false;
                for byte in &buffer[..count] {
                    if *byte == b'\n' {
                        if let Err(error) = dispatch_response(&reader_shared, &line) {
                            reader_shared.close(&error); failed = true; break;
                        }
                        line.clear();
                    } else if line.len() >= MAX_RESPONSE {
                        reader_shared.close("后台响应超过8MiB，连接已中断。"); failed = true; break;
                    } else { line.push(*byte); }
                }
                if failed { break; }
            }
        });
        let stderr_shared = shared.clone();
        let stderr = thread::spawn(move || {
            let mut buffer = [0_u8; 4096];
            while let Ok(count) = errors.read(&mut buffer) {
                if count == 0 { break; }
                let mut tail = stderr_shared.stderr.lock().unwrap();
                for byte in &buffer[..count] {
                    if tail.len() >= 16384 { tail.pop_front(); }
                    tail.push_back(*byte);
                }
            }
        });
        let monitor_shared = shared.clone();
        let monitor = thread::spawn(move || {
            let mut close_started = None;
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        monitor_shared.close(&format!("后台进程已结束：{status}"));
                        monitor_shared.finish(false, &status.to_string());
                        break;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        monitor_shared.close(&format!("无法查询后台进程状态：{error}"));
                    }
                }
                if !monitor_shared.alive.load(Ordering::Acquire) {
                    let started = close_started.get_or_insert_with(Instant::now);
                    if started.elapsed() >= EXIT_GRACE {
                        // Child拥有精确的子进程句柄；从不按名字或来自UI的PID杀进程。
                        let _ = child.kill();
                        if let Ok(status) = child.wait() {
                            monitor_shared.finish(true, &status.to_string());
                            break;
                        }
                        // 未确认wait成功不能宣布退出或允许另起子进程；保持拥有句柄并继续重试。
                    }
                }
                thread::sleep(Duration::from_millis(20));
            }
        });
        Ok(Arc::new(Self { shared, workspace, next_id: AtomicU64::new(1),
            threads: Mutex::new(vec![writer, reader, stderr, monitor]) }))
    }

    pub(crate) fn request(&self, mut request: Value) -> Result<Value, String> {
        if !self.shared.alive.load(Ordering::Acquire) { return Err("连接已失效，请重新连接；不要自动重试可能已执行的启动请求。".into()); }
        let object = request.as_object_mut().ok_or("请求必须是JSON对象")?;
        let op = object.get("op").and_then(Value::as_str).ok_or("请求缺少op")?;
        if !["capabilities", "nodes.list", "nodes.describe", "devices.list", "audio.inspect", "graph.validate", "tasks.start",
              "tasks.status", "tasks.cancel", "tasks.result", "tasks.release"].contains(&op) {
            return Err("未开放的控制操作".into());
        }
        let counter = self.next_id.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| "请求ID已耗尽，请重新连接")?;
        let id = format!("{}-{counter}", self.shared.session_id);
        object.insert("schema_version".into(), json!(1));
        object.insert("id".into(), Value::String(id.clone()));
        validate_request_depth(&request, 0)?;
        let mut bytes = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_REQUEST { return Err("请求超过4MiB".into()); }
        bytes.push(b'\n');
        let (sender, receiver) = mpsc::channel();
        {
            let mut pending = self.shared.pending.lock().unwrap();
            if !self.shared.alive.load(Ordering::Acquire) { return Err("连接已关闭".into()); }
            if pending.len() >= MAX_PENDING { return Err("待处理控制请求过多，请稍后重试查询。".into()); }
            pending.insert(id.clone(), sender);
        }
        let sent = self.shared.writer.lock().unwrap().as_ref()
            .ok_or_else(|| "后台写入通道已关闭".to_owned())
            .and_then(|sender| sender.try_send(bytes).map_err(|e| format!("后台写入队列不可用：{e}")));
        if let Err(error) = sent {
            self.shared.pending.lock().unwrap().remove(&id);
            return Err(error);
        }
        match receiver.recv_timeout(RPC_TIMEOUT) {
            Ok(reply) => reply,
            Err(_) => {
                let reason = "控制请求超时或连接失效，任务可能已开始；已关闭连接并请求取消，请勿自动重试启动。";
                let report = self.shutdown(reason);
                Err(match report { Ok(report) => report.message, Err(error) => format!("{reason} {error}") })
            }
        }
    }

    pub(crate) fn shutdown(&self, reason: &str) -> Result<DisconnectReport, String> {
        self.shared.close(reason);
        let guard = self.shared.exited.lock().unwrap();
        let (guard, _) = self.shared.exit_ready.wait_timeout_while(guard, Duration::from_secs(7), |exit| exit.is_none())
            .map_err(|_| "后台退出等待失败")?;
        guard.clone().ok_or_else(|| "后台进程尚未确认退出，暂时禁止重连。".into())
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.shutdown("连接关闭，已请求取消活动任务。");
        // 只有monitor确认进程退出后才回收管道线程；不会向未知进程发信号。
        if self.shared.exited.lock().unwrap().is_some() {
            for handle in self.threads.lock().unwrap().drain(..) { let _ = handle.join(); }
        }
    }
}

fn dispatch_response(shared: &Shared, bytes: &[u8]) -> Result<(), String> {
    let value: Value = serde_json::from_slice(bytes).map_err(|e| format!("后台JSON响应无效：{e}"))?;
    if value.get("schema_version").and_then(Value::as_u64) != Some(1) || !value.get("success").is_some_and(Value::is_boolean) {
        return Err("后台响应协议版本或success字段无效".into());
    }
    let id = value.get("id").and_then(Value::as_str).ok_or("后台响应缺少关联ID")?;
    if let Some(sender) = shared.pending.lock().unwrap().remove(id) { let _ = sender.send(Ok(value)); }
    // 已超时/失效请求的迟到响应不允许被误配给新的请求。
    Ok(())
}

fn validate_request_depth(value: &Value, depth: usize) -> Result<(), String> {
    if depth > 64 { return Err("请求JSON嵌套超过64层（包含协议外壳）".into()); }
    match value {
        Value::Array(items) => { for item in items { validate_request_depth(item, depth + 1)?; } }
        Value::Object(items) => { for item in items.values() { validate_request_depth(item, depth + 1)?; } }
        _ => {}
    }
    Ok(())
}

pub struct BackendManager {
    connection: Mutex<Option<Arc<Connection>>>,
    lifecycle: Mutex<()>,
    next_session: AtomicU64,
    pub closing: AtomicBool,
    pub shutdown_complete: AtomicBool,
}

impl Default for BackendManager {
    fn default() -> Self {
        Self { connection: Mutex::new(None), lifecycle: Mutex::new(()), next_session: AtomicU64::new(1),
            closing: AtomicBool::new(false), shutdown_complete: AtomicBool::new(false) }
    }
}

impl BackendManager {
    #[cfg(test)]
    pub(crate) fn with_test_connection(connection: Arc<Connection>) -> Self {
        let manager = Self::default();
        *manager.connection.lock().unwrap() = Some(connection);
        manager
    }

    pub fn connect(&self, workspace: String, allow_devices: bool, allow_monitor: bool,
                   callback: DisconnectCallback) -> Result<ConnectionInfo, String> {
        let _operation = self.lifecycle.lock().unwrap();
        if self.closing.load(Ordering::Acquire) { return Err("窗口正在关闭".into()); }
        if allow_monitor && !allow_devices { return Err("有声输出必须同时允许设备访问".into()); }
        let workspace = std::fs::canonicalize(workspace).map_err(|e| format!("工作目录无效：{e}"))?;
        if !workspace.is_dir() { return Err("工作区必须是已有目录".into()); }
        let previous = self.connection.lock().unwrap().clone();
        let mut previous_forced_disconnect = false;
        if let Some(previous) = previous {
            previous_forced_disconnect = previous.shutdown("重新连接前关闭旧会话，已请求取消活动任务。")?.forced;
        }
        self.connection.lock().unwrap().take();
        let id = self.next_session.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| "连接ID耗尽，请重启应用")?;
        let executable = fixed_sidecar()?;
        let connection = Connection::spawn(&executable, workspace.clone(), format!("session-{id}"),
            allow_devices, allow_monitor, callback)?;
        let capabilities = connection.request(json!({"op":"capabilities"}))?;
        if capabilities.get("success") != Some(&Value::Bool(true)) {
            let _ = connection.shutdown("后台能力握手失败。");
            return Err(format!("后台能力握手失败：{capabilities}"));
        }
        let result = ConnectionInfo { session_id: connection.shared.session_id.clone(),
            workspace: workspace.to_string_lossy().into_owned(), allow_devices, allow_monitor,
            capabilities: capabilities.get("data").cloned().ok_or("能力响应缺少data")?, previous_forced_disconnect };
        *self.connection.lock().unwrap() = Some(connection);
        Ok(result)
    }

    fn selected(&self, session_id: &str) -> Result<Arc<Connection>, String> {
        self.connection.lock().unwrap().as_ref().filter(|c| c.shared.session_id == session_id)
            .cloned().ok_or_else(|| "连接ID已失效；旧任务不能跨连接使用。".into())
    }

    pub fn request(&self, session_id: &str, request: Value) -> Result<Value, String> {
        let connection = self.selected(session_id)?;
        match connection.request(request) {
            Err(message) if connection.shared.alive.load(Ordering::Acquire) => {
                // 发送前的类型/大小/队列拒绝不意味着进程失联。保持连接并让UI按业务错误显示，
                // 避免UI丢掉会话而后台仍在执行先前任务。此请求没有被自动执行或重试。
                Ok(json!({"schema_version":1,"id":null,"success":false,"errors":[{
                    "code":"desktop_request_rejected",
                    "message":format!("请求在发送前被拒绝，连接保持有效；未自动执行或重试。{message}")
                }]}))
            }
            result => result,
        }
    }

    pub fn workspace(&self, session_id: &str) -> Result<PathBuf, String> {
        let selected = self.selected(session_id)?;
        if !selected.shared.alive.load(Ordering::Acquire) { return Err("连接已失效".into()); }
        Ok(selected.workspace.clone())
    }

    pub fn disconnect(&self, session_id: &str) -> Result<DisconnectReport, String> {
        let _operation = self.lifecycle.lock().unwrap();
        let selected = self.selected(session_id)?;
        let report = selected.shutdown("用户断开连接，已请求取消活动任务。")?;
        self.connection.lock().unwrap().take();
        Ok(report)
    }

    pub fn shutdown(&self) -> Result<DisconnectReport, String> {
        self.closing.store(true, Ordering::Release);
        let _operation = self.lifecycle.lock().unwrap();
        let selected = self.connection.lock().unwrap().clone();
        let report = if let Some(selected) = selected { selected.shutdown("窗口关闭，已请求取消活动任务。")? }
            else { DisconnectReport { forced: false, message: "没有后台连接。".into() } };
        self.connection.lock().unwrap().take();
        self.shutdown_complete.store(true, Ordering::Release);
        Ok(report)
    }
}

fn fixed_sidecar() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let directory = executable.parent().ok_or("无法定位应用目录")?;
    let filename = if cfg!(windows) { "control-cli.exe" } else { "control-cli" };
    let sidecar = directory.join(filename);
    if !sidecar.is_file() { return Err("缺少 control-cli Sidecar，请先构建C++并通过Tauri开发/构建流程复制二进制。".into()); }
    Ok(sidecar)
}

#[cfg(test)]
#[path = "backend_tests.rs"]
mod tests;
