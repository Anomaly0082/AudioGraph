//! Local, bounded audio decoding. Only this module constructs FFmpeg arguments.
use crate::tool_workspaces::{ToolWorkspaces, check_existing_chain, is_hardlinked, normal_metadata};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, atomic::{AtomicBool, AtomicU64, Ordering}};
use std::time::{Duration, Instant};

const INPUT_LIMIT: u64 = 512 * 1024 * 1024;
const OUTPUT_LIMIT: u64 = 256 * 1024 * 1024;
const AI_QUOTA: u64 = 512 * 1024 * 1024;
const STDERR_LIMIT: usize = 16 * 1024;
const PROBE_LIMIT: usize = 64 * 1024;
static CONVERSION: Mutex<()> = Mutex::new(());
static SERIAL: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ConversionOptions {
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    pub output_format: OutputFormat,
}

impl ConversionOptions {
    pub(crate) fn validate(&self) -> Result<(),String> {
        if self.sample_rate.is_some_and(|rate| !(8000..=192000).contains(&rate))
            || self.channels.is_some_and(|channels| !matches!(channels, 1 | 2)) {
            return Err("采样率须为 8000–192000，声道数须为 1 或 2".into());
        }
        if let Some(rate) = self.sample_rate {
            if !self.output_format.supports_rate(rate) {
                return Err(format!("{} 不支持 {} Hz；请选择兼容格式或显式指定兼容采样率", self.output_format.name().to_uppercase(), rate));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum OutputFormat { #[default] Wav, Flac, Mp3, M4a }
impl OutputFormat {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value { "wav" => Ok(Self::Wav), "flac" => Ok(Self::Flac), "mp3" => Ok(Self::Mp3), "m4a" => Ok(Self::M4a),
            _ => Err("输出格式仅支持 wav、flac、mp3、m4a".into()) }
    }
    fn name(self) -> &'static str { match self { Self::Wav=>"wav", Self::Flac=>"flac", Self::Mp3=>"mp3", Self::M4a=>"m4a" } }
    fn codec(self) -> &'static str { match self { Self::Wav=>"pcm_s16le", Self::Flac=>"flac", Self::Mp3=>"mp3", Self::M4a=>"aac" } }
    fn demuxer(self) -> &'static str { if self == Self::M4a { "mov" } else { self.name() } }
    fn supports_rate(self, rate: u32) -> bool {
        match self {
            Self::Mp3 => [8000,11025,12000,16000,22050,24000,32000,44100,48000].contains(&rate),
            Self::M4a => [8000,11025,12000,16000,22050,24000,32000,44100,48000,64000,88200,96000].contains(&rate),
            _ => (8000..=192000).contains(&rate),
        }
    }
}

fn io(error: std::io::Error) -> String { format!("音频转换失败：{error}") }
fn checkpoint(cancel: &AtomicBool, deadline: Instant) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) { return Err("音频转换已取消".into()); }
    if Instant::now() >= deadline { return Err("音频转换超时，未生成目标文件".into()); }
    Ok(())
}
#[cfg(test)]
fn path_key(path: &Path) -> Vec<String> {
    let text = path.to_string_lossy();
    let normal = if let Some(rest) = text.strip_prefix("\\\\?\\UNC\\") { format!("\\\\{rest}") }
        else if let Some(rest) = text.strip_prefix("\\\\?\\") { rest.to_owned() } else { text.into_owned() };
    Path::new(&normal).components().map(|p| p.as_os_str().to_string_lossy().to_lowercase()).collect()
}
#[cfg(test)]
fn contained(root: &Path, path: &Path) -> bool { path_key(path).starts_with(&path_key(root)) }

#[cfg(test)]
fn trusted_executable(spaces: &ToolWorkspaces, path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() || path.components().any(|p| matches!(p, std::path::Component::ParentDir | std::path::Component::CurDir)) {
        return Err("FFmpeg 路径必须是规范的绝对路径".into());
    }
    check_existing_chain(path)?;
    let meta = normal_metadata(path)?.ok_or("找不到 FFmpeg 可执行文件")?;
    if !meta.is_file() || is_hardlinked(path, &meta) { return Err("FFmpeg 必须是独立的普通可执行文件".into()); }
    let path = fs::canonicalize(path).map_err(io)?;
    if contained(&spaces.user_root, &path) || contained(&spaces.ai_root, &path)
        || std::env::current_dir().is_ok_and(|cwd| contained(&cwd, &path)) {
        return Err("不能运行工作区或当前目录内的 FFmpeg，请安装到独立目录".into());
    }
    #[cfg(windows)]
    if !path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("exe")) {
        return Err("FFmpeg 必须是 .exe 可执行文件".into());
    }
    Ok(path)
}

pub(crate) fn locate_ffmpeg(_spaces: &ToolWorkspaces) -> Result<PathBuf, String> {
    // Production and development resolve the same adjacent, replaceable runtime.
    // No PATH/current-directory/environment fallback can select a different binary.
    crate::ffmpeg_bundle::locate_bundled()
}

fn demuxer(path: &str) -> Result<&'static str, String> {
    match Path::new(path).extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
        "mp3" => Ok("mp3"), "m4a" => Ok("mov"), "aac" => Ok("aac"), "flac" => Ok("flac"),
        "ogg" | "oga" => Ok("ogg"), "wav" => Ok("wav"), "webm" => Ok("matroska"),
        _ => Err("仅支持 MP3、M4A、AAC、FLAC、OGG、WAV 和 WebM 音频".into()),
    }
}

struct Stage(PathBuf);
impl Stage {
    fn new(parent: &Path) -> Result<Self, String> {
        for _ in 0..64 {
            let path = parent.join(format!(".audio-convert-{}-{}", std::process::id(), SERIAL.fetch_add(1, Ordering::Relaxed)));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(io(error)),
            }
        }
        Err("无法创建转换临时目录".into())
    }
}
impl Drop for Stage { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

struct Process(Child);
impl Drop for Process { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }

fn snapshot(source: &Path, source_root: &Path, destination: &Path, cancel: &AtomicBool, deadline: Instant) -> Result<File, String> {
    let before = normal_metadata(source)?.ok_or("源音频已不存在")?;
    if !before.is_file() || is_hardlinked(source, &before) { return Err("源音频必须是普通文件，不支持链接".into()); }
    if before.len() == 0 || before.len() > INPUT_LIMIT { return Err("源音频为空或超过 512 MiB".into()); }
    let mut options = OpenOptions::new(); options.read(true);
    #[cfg(windows)] { use std::os::windows::fs::OpenOptionsExt; options.share_mode(1); }
    let mut source_file = options.open(source).map_err(io)?;
    let opened = source_file.metadata().map_err(io)?;
    if !crate::run_records::opened_file_within_workspace(&source_file, source_root)
        || opened.len() != before.len() || opened.modified().ok() != before.modified().ok() {
        return Err("源音频已变化或越界，请刷新重试".into());
    }
    let mut output = OpenOptions::new().write(true).create_new(true).open(destination).map_err(io)?;
    let mut total = 0u64; let mut buffer = [0u8; 64 * 1024];
    loop {
        checkpoint(cancel, deadline)?;
        let read = source_file.read(&mut buffer).map_err(io)?;
        if read == 0 { break; }
        total += read as u64;
        if total > INPUT_LIMIT { return Err("源音频超过 512 MiB".into()); }
        output.write_all(&buffer[..read]).map_err(io)?;
    }
    output.sync_all().map_err(io)?;
    let after = source_file.metadata().map_err(io)?;
    if total != before.len() || after.len() != before.len() || after.modified().ok() != before.modified().ok()
        || !crate::run_records::opened_file_within_workspace(&source_file, source_root) {
        return Err("读取期间源音频已变化，未生成目标文件".into());
    }
    Ok(source_file) // Keep the Windows read-only sharing lease until publication.
}

fn command(executable: &Path, input: &Path, output: &Path, format: &str, options: ConversionOptions) -> Command {
    let mut command = Command::new(executable);
    #[cfg(windows)] { use std::os::windows::process::CommandExt; command.creation_flags(0x08000000); }
    command.current_dir(input.parent().expect("private stage parent"))
        .env_remove("FFREPORT").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped())
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-xerror", "-n", "-max_alloc", "67108864",
            "-protocol_whitelist", "file,pipe", "-format_whitelist", "mp3,mov,aac,flac,ogg,wav,matroska,webm",
            "-probesize", "8388608", "-analyzeduration", "10000000", "-threads", "1", "-filter_threads", "1", "-f", format]);
    if format == "mov" { command.args(["-enable_drefs", "0", "-use_absolute_path", "0"]); }
    command.arg("-i").arg(input).args(["-map", "0:a:0", "-vn", "-sn", "-dn", "-map_metadata", "-1", "-map_chapters", "-1", "-threads", "1"]);
    match options.output_format {
        OutputFormat::Wav => { command.args(["-c:a","pcm_s16le","-f","wav","-rf64","never"]); }
        OutputFormat::Flac => { command.args(["-c:a","flac","-compression_level","5","-f","flac"]); }
        OutputFormat::Mp3 => { command.args(["-c:a","libmp3lame","-b:a","192k","-f","mp3"]); }
        OutputFormat::M4a => { command.args(["-c:a","aac","-b:a","192k","-f","ipod","-movflags","+faststart"]); }
    }
    if let Some(rate) = options.sample_rate { command.arg("-ar").arg(rate.to_string()); }
    if let Some(channels) = options.channels { command.arg("-ac").arg(channels.to_string()); }
    command.arg(output);
    command
}

fn run(executable: &Path, input: &Path, output: &Path, format: &str, options: ConversionOptions,
    cancel: &AtomicBool, deadline: Instant, output_limit: u64) -> Result<(), String> {
    let mut child = Process(command(executable, input, output, format, options).spawn().map_err(io)?);
    let mut stderr = child.0.stderr.take().ok_or("无法读取 FFmpeg 错误输出")?;
    let reader = std::thread::spawn(move || {
        let mut retained = Vec::new(); let mut buffer = [0u8; 4096];
        loop { match stderr.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(count) => { let keep = count.min(STDERR_LIMIT.saturating_sub(retained.len())); retained.extend_from_slice(&buffer[..keep]); },
        }}
        String::from_utf8_lossy(&retained).into_owned()
    });
    let result = loop {
        if let Err(error) = checkpoint(cancel, deadline) { break Err(error); }
        if fs::metadata(output).is_ok_and(|meta| meta.len() > output_limit) { break Err("转换输出超过 256 MiB，未生成目标文件".into()); }
        match child.0.try_wait() {
            Ok(Some(status)) => break if status.success() { Ok(()) } else { Err("FFmpeg 无法解码此音频".into()) },
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => break Err(io(error)),
        }
    };
    // Close process handles before joining the always-draining stderr reader.
    drop(child);
    let stderr = reader.join().unwrap_or_default();
    result.map_err(|error| if stderr.trim().is_empty() { error } else { format!("{error}：{}", stderr.trim()) })?;
    checkpoint(cancel, deadline)?;
    if fs::metadata(output).map_err(io)?.len() > output_limit { return Err("转换输出超过 256 MiB，未生成目标文件".into()); }
    Ok(())
}

#[derive(Debug)]
struct MediaInfo { rate: u32, channels: u16, codec: String, format: String, duration: Option<f64>, bits: Option<u32>, bit_rate: Option<u64> }

fn drain(mut pipe: impl Read, limit: usize) -> (Vec<u8>, bool) {
    let mut kept = Vec::new(); let mut buffer = [0u8; 4096]; let mut truncated = false;
    loop { match pipe.read(&mut buffer) {
        Ok(0) | Err(_) => break,
        Ok(count) => {
            let n = count.min(limit.saturating_sub(kept.len()));
            kept.extend_from_slice(&buffer[..n]); truncated |= n < count;
        }
    }}
    (kept, truncated)
}

fn probe(executable: &Path, path: &Path, format: &str, output: bool, cancel: &AtomicBool, deadline: Instant) -> Result<MediaInfo, String> {
    checkpoint(cancel, deadline)?;
    let ffprobe = crate::ffmpeg_bundle::locate_bundled_probe()?;
    if ffprobe.parent() != executable.parent() { return Err("FFmpeg 和 FFprobe 不属于同一个运行库目录".into()); }
    check_existing_chain(&ffprobe)?;
    if !normal_metadata(&ffprobe)?.is_some_and(|meta| meta.is_file() && !is_hardlinked(&ffprobe, &meta)) {
        return Err("随附 FFprobe 缺失，请重新安装完整程序包".into());
    }
    let mut command = Command::new(ffprobe);
    #[cfg(windows)] { use std::os::windows::process::CommandExt; command.creation_flags(0x08000000); }
    command.current_dir(path.parent().ok_or("媒体路径无效")?).env_remove("FFREPORT")
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .args(["-hide_banner","-loglevel","error","-max_alloc","67108864",
            "-protocol_whitelist","file,pipe","-format_whitelist","mp3,mov,aac,flac,ogg,wav,matroska,webm",
            "-probesize","8388608","-analyzeduration","10000000","-f",format]);
    if format == "mov" { command.args(["-enable_drefs","0","-use_absolute_path","0"]); }
    if !output { command.args(["-select_streams","a:0"]); }
    command.args(["-show_entries","stream=codec_type,codec_name,sample_rate,channels,bits_per_raw_sample,bits_per_sample,bit_rate:format=format_name,duration,size","-of","json","-i"]).arg(path);
    let mut child = Process(command.spawn().map_err(io)?);
    let stdout = child.0.stdout.take().ok_or("无法读取媒体信息")?;
    let stderr = child.0.stderr.take().ok_or("无法读取媒体检查错误")?;
    let out_reader = std::thread::spawn(move || drain(stdout, PROBE_LIMIT));
    let err_reader = std::thread::spawn(move || drain(stderr, STDERR_LIMIT));
    let result = loop {
        if let Err(error) = checkpoint(cancel, deadline) { break Err(error); }
        match child.0.try_wait() {
            Ok(Some(status)) => break if status.success() { Ok(()) } else { Err("无法读取音频格式信息".into()) },
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => break Err(io(error)),
        }
    };
    drop(child);
    let (bytes, truncated) = out_reader.join().map_err(|_| "媒体信息读取失败")?;
    let (errors, _) = err_reader.join().map_err(|_| "媒体错误读取失败")?;
    result.map_err(|error| format!("{error}：{}", String::from_utf8_lossy(&errors).trim()))?;
    checkpoint(cancel, deadline)?;
    if truncated { return Err("音频格式信息过大".into()); }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| "媒体信息不是有效 JSON")?;
    let streams = value["streams"].as_array().filter(|s| s.len() == 1).ok_or("需要一条可识别的音轨")?;
    let stream = &streams[0];
    if stream["codec_type"] != "audio" { return Err("文件没有可转换音轨".into()); }
    let rate = stream["sample_rate"].as_str().and_then(|s| s.parse::<u32>().ok()).filter(|v| *v > 0).ok_or("采样率无效")?;
    let channels = stream["channels"].as_u64().and_then(|v| u16::try_from(v).ok()).filter(|v| *v > 0).ok_or("声道数无效")?;
    let codec = stream["codec_name"].as_str().ok_or("编码信息缺失")?.to_owned();
    let format = value["format"]["format_name"].as_str().ok_or("容器信息缺失")?.to_owned();
    let duration = value["format"]["duration"].as_str().and_then(|s| s.parse::<f64>().ok()).filter(|v| v.is_finite() && *v > 0.);
    let bits = stream["bits_per_raw_sample"].as_str().and_then(|s| s.parse::<u32>().ok()).filter(|v| *v > 0)
        .or_else(|| stream["bits_per_sample"].as_u64().and_then(|v| u32::try_from(v).ok()).filter(|v| *v > 0));
    if output && value["format"]["size"].as_str().and_then(|s| s.parse::<u64>().ok()) != Some(fs::metadata(path).map_err(io)?.len()) {
        return Err("转换文件长度与格式信息不一致".into());
    }
    let bit_rate = stream["bit_rate"].as_str().and_then(|s| s.parse::<u64>().ok()).filter(|v| *v > 0);
    Ok(MediaInfo { rate, channels, codec, format, duration, bits, bit_rate })
}

#[derive(Debug)]
struct Wave { sample_rate: u32, channels: u16, frames: u64, bytes: u64, data_offset: u64, data_bytes: u64 }
fn validate_wave(path: &Path, options: ConversionOptions) -> Result<Wave, String> {
    let mut file = File::open(path).map_err(io)?;
    let bytes = file.metadata().map_err(io)?.len();
    let invalid = || "转换结果不是完整的 PCM16 WAV 音频".to_owned();
    if bytes < 44 || bytes > OUTPUT_LIMIT { return Err(invalid()); }
    let mut header = [0u8; 12]; file.read_exact(&mut header).map_err(io)?;
    if &header[..4] != b"RIFF" || &header[8..] != b"WAVE" || u32::from_le_bytes(header[4..8].try_into().unwrap()) as u64 + 8 != bytes { return Err(invalid()); }
    let mut position = 12u64; let mut format = None; let mut data = None; let mut chunks = 0usize;
    while position < bytes {
        chunks += 1; if chunks > 128 || bytes - position < 8 { return Err(invalid()); }
        let mut chunk = [0u8; 8]; file.read_exact(&mut chunk).map_err(io)?;
        let size = u32::from_le_bytes(chunk[4..].try_into().unwrap()) as u64;
        let end = position.checked_add(8).and_then(|p| p.checked_add(size)).ok_or_else(invalid)?;
        let next = end.checked_add(size % 2).ok_or_else(invalid)?;
        if next > bytes { return Err(invalid()); }
        if &chunk[..4] == b"fmt " {
            if format.is_some() || !(16..=64).contains(&size) { return Err(invalid()); }
            let mut fmt = vec![0u8; size as usize]; file.read_exact(&mut fmt).map_err(io)?;
            let tag = u16::from_le_bytes(fmt[0..2].try_into().unwrap());
            let channels = u16::from_le_bytes(fmt[2..4].try_into().unwrap());
            let rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
            let byte_rate = u32::from_le_bytes(fmt[8..12].try_into().unwrap());
            let align = u16::from_le_bytes(fmt[12..14].try_into().unwrap());
            let bits = u16::from_le_bytes(fmt[14..16].try_into().unwrap());
            let pcm = tag == 1 || (tag == 0xfffe && size >= 40 && fmt[16..18] == [22, 0]
                && fmt[18..20] == [16, 0] && fmt[24..40] == [1,0,0,0,0,0,16,0,128,0,0,170,0,56,155,113]);
            if !pcm || bits != 16 || !matches!(channels, 1 | 2) || !(8000..=192000).contains(&rate)
                || align != channels * 2 || byte_rate != rate * align as u32 {
                return Err("输出需为 8–192 kHz 单声道或双声道 PCM16 WAV；请指定采样率或声道数后重试".into());
            }
            format = Some((rate, channels, align));
        } else if &chunk[..4] == b"data" { if data.replace((position + 8, size)).is_some() { return Err(invalid()); } }
        file.seek(SeekFrom::Start(next)).map_err(io)?; position = next;
    }
    let (sample_rate, channels, align) = format.ok_or_else(invalid)?;
    let (data_offset, data) = data.ok_or_else(invalid)?;
    if data == 0 || data % align as u64 != 0 || options.sample_rate.is_some_and(|rate| rate != sample_rate)
        || options.channels.is_some_and(|count| count != channels) { return Err(invalid()); }
    Ok(Wave { sample_rate, channels, frames: data / align as u64, bytes, data_offset, data_bytes: data })
}

// FFmpeg can emit WAVE_FORMAT_EXTENSIBLE at high rates. The graph reader accepts
// only classic tag 1 PCM. Copy validated PCM samples into a minimal tag 1 RIFF,
// preserving rate, channels and every sample without resampling or re-encoding.
fn canonical_wave(source: &Path, destination: &Path, wave: &Wave, cancel: &AtomicBool, deadline: Instant) -> Result<Wave, String> {
    let mut input = File::open(source).map_err(io)?;
    input.seek(SeekFrom::Start(wave.data_offset)).map_err(io)?;
    let mut output = OpenOptions::new().write(true).create_new(true).open(destination).map_err(io)?;
    let mut header = Vec::with_capacity(44);
    header.extend_from_slice(b"RIFF"); header.extend_from_slice(&(36 + wave.data_bytes as u32).to_le_bytes());
    header.extend_from_slice(b"WAVEfmt "); header.extend_from_slice(&16u32.to_le_bytes());
    header.extend_from_slice(&1u16.to_le_bytes()); header.extend_from_slice(&wave.channels.to_le_bytes());
    header.extend_from_slice(&wave.sample_rate.to_le_bytes()); header.extend_from_slice(&(wave.sample_rate * wave.channels as u32 * 2).to_le_bytes());
    header.extend_from_slice(&(wave.channels * 2).to_le_bytes()); header.extend_from_slice(&16u16.to_le_bytes());
    header.extend_from_slice(b"data"); header.extend_from_slice(&(wave.data_bytes as u32).to_le_bytes());
    output.write_all(&header).map_err(io)?;
    let mut remaining = wave.data_bytes; let mut buffer = [0u8; 64 * 1024];
    while remaining > 0 {
        checkpoint(cancel, deadline)?;
        let count = remaining.min(buffer.len() as u64) as usize;
        input.read_exact(&mut buffer[..count]).map_err(io)?;
        output.write_all(&buffer[..count]).map_err(io)?;
        remaining -= count as u64;
    }
    output.sync_all().map_err(io)?; drop(output);
    validate_wave(destination, ConversionOptions::default())
}

fn quota(root: &Path, excluded: Option<&Path>, cancel: &AtomicBool, deadline: Instant) -> Result<u64, String> {
    fn walk(path: &Path, excluded: Option<&Path>, depth: usize, entries: &mut usize, bytes: &mut u64,
        cancel: &AtomicBool, deadline: Instant) -> Result<(), String> {
        checkpoint(cancel, deadline)?;
        if excluded == Some(path) { return Ok(()); }
        *entries += 1; if *entries > 10000 || depth > 16 { return Err("AI 工作区过大，无法安全检查配额".into()); }
        check_existing_chain(path)?;
        let meta = normal_metadata(path)?.ok_or("AI 工作区已变化")?;
        if meta.is_dir() { for entry in fs::read_dir(path).map_err(io)? { walk(&entry.map_err(io)?.path(), excluded, depth + 1, entries, bytes, cancel, deadline)?; } }
        else if meta.is_file() && !is_hardlinked(path, &meta) { *bytes = bytes.checked_add(meta.len()).ok_or("AI 工作区配额已满")?; }
        else { return Err("AI 工作区含不支持的链接或文件".into()); }
        if *bytes > AI_QUOTA { return Err("AI 工作区超过 512 MiB 配额".into()); } Ok(())
    }
    let mut bytes = 0; walk(root, excluded, 0, &mut 0, &mut bytes, cancel, deadline)?; Ok(bytes)
}

#[cfg(windows)]
fn publish(source: &Path, target: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name="Kernel32")]
    unsafe extern "system" { fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32; }
    let from: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<_> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 8) } == 0 { return Err(io(std::io::Error::last_os_error())); } Ok(())
}
#[cfg(not(windows))]
fn publish(source: &Path, target: &Path) -> Result<(), String> { fs::hard_link(source, target).map_err(io)?; fs::remove_file(source).map_err(io) }

pub(crate) fn convert(spaces: &ToolWorkspaces, source_space: &str, source_path: &str, target_space: &str,
    path: &str, mut options: ConversionOptions, cancel: &AtomicBool, deadline: Instant) -> Result<Value, String> {
    let deadline = deadline.min(Instant::now() + Duration::from_secs(120));
    let _lease = loop {
        checkpoint(cancel, deadline)?;
        match CONVERSION.try_lock() {
            Ok(lease) => break lease,
            Err(std::sync::TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return Err("转换服务暂不可用，请重启应用".into()),
        }
    };
    options.validate()?;
    if source_path.is_empty() || path.is_empty() { return Err("请选择源音频和新的目标文件名".into()); }
    let format = demuxer(source_path)?;
    if !Path::new(path).extension().is_some_and(|extension| extension.eq_ignore_ascii_case(options.output_format.name())) {
        return Err(format!("目标文件名必须与输出格式一致，以 .{} 结尾", options.output_format.name()));
    }
    let source = spaces.checked_path(source_space, source_path, false)?;
    let target = spaces.checked_path(target_space, path, true)?;
    if normal_metadata(&target)?.is_some() { return Err("目标已存在，请选择其他名称；不会覆盖文件".into()); }
    if target_space == "ai" && path.split('/').count().saturating_sub(1) > 3 { return Err("AI 目标文件夹最多支持 3 层".into()); }
    let parent = target.parent().ok_or("目标没有父目录")?;
    if !normal_metadata(parent)?.is_some_and(|meta| meta.is_dir()) { return Err("目标文件夹不存在，请先创建文件夹".into()); }
    if target_space == "ai" { quota(&spaces.ai_root, None, cancel, deadline)?; }
    let executable = locate_ffmpeg(spaces)?;
    let stage = Stage::new(parent)?;
    let input = stage.0.join("input.snapshot"); let output = stage.0.join(format!("output.{}", options.output_format.name()));
    let root = if source_space == "user" { &spaces.user_root } else { &spaces.ai_root };
    let _source_lease = snapshot(&source, root, &input, cancel, deadline)?;
    let source_info = probe(&executable, &input, format, false, cancel, deadline)?;
    let rate = options.sample_rate.unwrap_or(source_info.rate);
    let channels = options.channels.unwrap_or(source_info.channels);
    if !(8000..=192000).contains(&rate) || !matches!(channels, 1 | 2) {
        return Err("当前转换支持 8–192 kHz 单/双声道；需要时请显式指定采样率或声道数".into());
    }
    if !options.output_format.supports_rate(rate) {
        return Err(format!("{} 不支持 {} Hz；请选择兼容格式或显式指定兼容采样率", options.output_format.name().to_uppercase(), rate));
    }
    // Explicitly pin the desired rate/layout so lossy encoders cannot silently resample.
    options.sample_rate = Some(rate); options.channels = Some(channels);
    run(&executable, &input, &output, format, options, cancel, deadline, OUTPUT_LIMIT)?;
    let (publication, mut result, bytes) = if options.output_format == OutputFormat::Wav {
        let decoded = validate_wave(&output, options)?;
        let canonical = stage.0.join("canonical.wav");
        let wave = canonical_wave(&output, &canonical, &decoded, cancel, deadline)?;
        let result = json!({"encoding":"pcm_s16le","sample_rate":wave.sample_rate,"channels":wave.channels,
            "frames":wave.frames,"duration_seconds":wave.frames as f64 / wave.sample_rate as f64,"bits_per_sample":16});
        (canonical, result, wave.bytes)
    } else {
        let info = probe(&executable, &output, options.output_format.demuxer(), true, cancel, deadline)?;
        if info.codec != options.output_format.codec() || !info.format.split(',').any(|f| f == options.output_format.demuxer())
            || info.rate != rate || info.channels != channels || info.duration.is_none() {
            return Err("转换结果的容器、编码或音频参数不符合请求，未发布文件".into());
        }
        let bytes = fs::metadata(&output).map_err(io)?.len();
        if bytes == 0 || bytes > OUTPUT_LIMIT { return Err("转换输出为空或超过 256 MiB，未发布文件".into()); }
        // Encoded packet counts are not PCM sample frames (AAC/MP3 have encoder delay).
        let result = json!({"encoding":info.codec,"sample_rate":info.rate,"channels":info.channels,
            "frames":null,"duration_seconds":info.duration,"bits_per_sample":info.bits,"bit_rate":info.bit_rate});
        (output, result, bytes)
    };
    if target_space == "ai" && quota(&spaces.ai_root, Some(&stage.0), cancel, deadline)?.checked_add(bytes).is_none_or(|size| size > AI_QUOTA) {
        return Err("转换结果将超过 AI 工作区 512 MiB 配额".into());
    }
    checkpoint(cancel, deadline)?;
    spaces.checked_path(source_space, source_path, false)?;
    spaces.checked_path(target_space, path, true)?;
    if normal_metadata(&target)?.is_some() { return Err("目标已存在，转换结果未写入".into()); }
    publish(&publication, &target)?;
    result["space"] = json!(target_space); result["path"] = json!(path);
    result["output_format"] = json!(options.output_format.name()); result["bytes"] = json!(bytes);
    Ok(result)
}

#[cfg(test)]
#[path="audio_conversion_tests.rs"]
mod tests;
#[cfg(test)]
#[path="audio_conversion_review_tests.rs"]
mod review_tests;
