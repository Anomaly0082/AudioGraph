use super::*;

struct Fixture { root: PathBuf, spaces: ToolWorkspaces }
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("audio-convert-test-{}-{}", std::process::id(), SERIAL.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&root).unwrap();
        let user_root = root.join("user"); let ai_root = root.join("ai");
        fs::create_dir(&user_root).unwrap(); fs::create_dir(&ai_root).unwrap();
        Self { root, spaces: ToolWorkspaces { user_root, ai_root } }
    }
    fn wav(&self, name: &str, rate: u32, channels: u16, frames: u32) -> PathBuf {
        let path = self.spaces.user_root.join(name);
        write_wave(&path, rate, channels, frames);
        path
    }
    fn convert(&self, source: &str, target: &str, options: ConversionOptions) -> Result<Value, String> {
        convert(&self.spaces, "user", source, "ai", target, options, &AtomicBool::new(false), Instant::now() + Duration::from_secs(20))
    }
    fn no_stages(&self) {
        for root in [&self.spaces.user_root, &self.spaces.ai_root] {
            assert!(!fs::read_dir(root).unwrap().any(|entry| entry.unwrap().file_name().to_string_lossy().starts_with(".audio-convert-")));
        }
    }
}
impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.root); } }

fn write_wave(path: &Path, rate: u32, channels: u16, frames: u32) {
    let bytes = frames * channels as u32 * 2;
    let mut file = File::create(path).unwrap();
    file.write_all(b"RIFF").unwrap(); file.write_all(&(bytes + 36).to_le_bytes()).unwrap();
    file.write_all(b"WAVEfmt ").unwrap(); file.write_all(&16u32.to_le_bytes()).unwrap();
    file.write_all(&1u16.to_le_bytes()).unwrap(); file.write_all(&channels.to_le_bytes()).unwrap();
    file.write_all(&rate.to_le_bytes()).unwrap(); file.write_all(&(rate * channels as u32 * 2).to_le_bytes()).unwrap();
    file.write_all(&(channels * 2).to_le_bytes()).unwrap(); file.write_all(&16u16.to_le_bytes()).unwrap();
    file.write_all(b"data").unwrap(); file.write_all(&bytes.to_le_bytes()).unwrap();
    let buffer = [0u8; 65536]; let mut remaining = bytes;
    while remaining > 0 { let count = remaining.min(buffer.len() as u32) as usize; file.write_all(&buffer[..count]).unwrap(); remaining -= count as u32; }
}

#[test]
fn pcm_output_is_classic_tag_one_and_preserves_high_rate_samples() {
    let f = Fixture::new();
    for rate in [8000, 44100, 96000, 192000] {
        let source = format!("source-{rate}.wav"); let destination = format!("目标-{rate}.wav");
        f.wav(&source, rate, 2, 1000);
        let before = fs::read(f.spaces.user_root.join(&source)).unwrap();
        let result = f.convert(&source, &destination, ConversionOptions::default()).unwrap();
        assert_eq!(result["sample_rate"], rate); assert_eq!(result["channels"], 2); assert_eq!(result["frames"], 1000);
        let output = fs::read(f.spaces.ai_root.join(&destination)).unwrap();
        assert_eq!(&output[20..22], &1u16.to_le_bytes()); assert_eq!(&output[44..], &before[44..]);
        assert_eq!(fs::read(f.spaces.user_root.join(&source)).unwrap(), before);
    }
    f.no_stages();
}

#[test]
fn local_compressed_formats_decode_without_modifying_sources() {
    let f = Fixture::new(); let source = f.wav("original.wav", 16000, 1, 1600);
    let executable = locate_ffmpeg(&f.spaces).unwrap();
    for (extension, codec) in [("mp3", "libmp3lame"), ("m4a", "aac"), ("aac", "aac"), ("flac", "flac"), ("ogg", "libvorbis"), ("webm", "libopus")] {
        let name = format!("sample.{extension}"); let encoded = f.spaces.user_root.join(&name);
        let mut encode = Command::new(&executable);
        #[cfg(windows)] { use std::os::windows::process::CommandExt; encode.creation_flags(0x08000000); }
        let result = encode.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-n", "-i"]).arg(&source)
            .args(["-c:a", codec]).arg(&encoded).stdin(Stdio::null()).output().unwrap();
        assert!(result.status.success(), "fixture {extension}: {}", String::from_utf8_lossy(&result.stderr));
        let before = fs::read(&encoded).unwrap();
        let result = f.convert(&name, &format!("{extension}.wav"), ConversionOptions::default()).unwrap();
        assert_eq!(result["encoding"], "pcm_s16le"); assert!(result["frames"].as_u64().unwrap() > 0);
        assert_eq!(fs::read(encoded).unwrap(), before);
    }
    f.no_stages();
}

#[test]
fn explicit_resampling_and_downmix_are_supported_but_incompatible_default_is_rejected() {
    let f = Fixture::new(); f.wav("surround.wav", 48000, 6, 4800);
    assert!(f.convert("surround.wav", "unsupported.wav", ConversionOptions::default()).is_err());
    assert!(!f.spaces.ai_root.join("unsupported.wav").exists());
    let result = f.convert("surround.wav", "mono.wav", ConversionOptions { sample_rate: Some(16000), channels: Some(1), ..Default::default() }).unwrap();
    assert_eq!(result["sample_rate"], 16000); assert_eq!(result["channels"], 1);
    f.no_stages();
}

#[test]
fn high_rate_stereo_can_be_explicitly_adapted_for_mp3_without_changing_source() {
    let f = Fixture::new(); let source = f.wav("high-rate.wav", 96000, 2, 9600);
    let original = fs::read(&source).unwrap();
    let options = ConversionOptions { output_format: OutputFormat::Mp3, ..Default::default() };
    assert!(f.convert("high-rate.wav", "unchanged-rate.mp3", options).unwrap_err().contains("96000"));
    assert!(!f.spaces.ai_root.join("unchanged-rate.mp3").exists());
    let result = f.convert("high-rate.wav", "adapted.mp3", ConversionOptions {
        sample_rate: Some(48000), channels: Some(1), ..options
    }).unwrap();
    assert_eq!(result["sample_rate"], 48000);
    assert_eq!(result["channels"], 1);
    assert_eq!(result["encoding"], "mp3");
    assert_eq!(fs::read(&source).unwrap(), original);
    f.no_stages();
}

#[test]
fn rejects_conflicts_paths_links_invalid_options_and_limits_without_publication() {
    let f = Fixture::new(); let source = f.wav("source.wav", 16000, 1, 100);
    fs::write(f.spaces.ai_root.join("existing.wav"), b"preserve").unwrap();
    for target in ["existing.wav", "../outside.wav", "", "result.mp3", "missing/result.wav", "a/b/c/d/result.wav"] {
        assert!(f.convert("source.wav", target, ConversionOptions::default()).is_err(), "{target}");
    }
    assert!(f.convert("source.wav", "rate.wav", ConversionOptions { sample_rate: Some(7999), channels: None, ..Default::default() }).is_err());
    assert!(f.convert("source.wav", "channels.wav", ConversionOptions { sample_rate: None, channels: Some(3), ..Default::default() }).is_err());
    fs::hard_link(&source, f.spaces.user_root.join("hard.wav")).unwrap();
    assert!(f.convert("hard.wav", "hard-result.wav", ConversionOptions::default()).is_err());
    assert_eq!(fs::read(f.spaces.ai_root.join("existing.wav")).unwrap(), b"preserve");
    let sparse = File::create(f.spaces.ai_root.join("quota")).unwrap(); sparse.set_len(AI_QUOTA + 1).unwrap();
    assert!(f.convert("source.wav", "quota-result.wav", ConversionOptions::default()).is_err());
    f.no_stages();
}

#[test]
fn corrupt_audio_and_playlist_disguises_never_publish() {
    let f = Fixture::new();
    for (name, bytes) in [("corrupt.mp3", b"not audio".as_slice()), ("playlist.mp3", b"#EXTM3U\nhttp://127.0.0.1:9/private\n".as_slice()), ("external.m4a", b"not a mov container".as_slice())] {
        fs::write(f.spaces.user_root.join(name), bytes).unwrap();
        assert!(f.convert(name, &format!("{name}.wav"), ConversionOptions::default()).is_err());
        assert!(!f.spaces.ai_root.join(format!("{name}.wav")).exists());
    }
    f.no_stages();
}

#[test]
fn cancellation_deadlines_and_output_limit_clean_up_without_truncation_success() {
    let f = Fixture::new(); let source = f.wav("source.wav", 16000, 1, 320000);
    let cancel = AtomicBool::new(true);
    assert!(convert(&f.spaces, "user", "source.wav", "ai", "cancel.wav", ConversionOptions::default(), &cancel, Instant::now() + Duration::from_secs(20)).is_err());
    assert!(convert(&f.spaces, "user", "source.wav", "ai", "deadline.wav", ConversionOptions::default(), &AtomicBool::new(false), Instant::now()).is_err());
    let stage = Stage::new(&f.spaces.ai_root).unwrap(); let output = stage.0.join("limited.wav");
    assert!(run(&locate_ffmpeg(&f.spaces).unwrap(), &source, &output, "wav", ConversionOptions::default(), &AtomicBool::new(false), Instant::now() + Duration::from_secs(20), 1024).is_err());
    drop(stage);
    assert!(!f.spaces.ai_root.join("cancel.wav").exists()); assert!(!f.spaces.ai_root.join("deadline.wav").exists());
    f.no_stages();
}

#[test]
fn cancellation_kills_and_reaps_an_active_local_decoder() {
    let f = Fixture::new();
    let source = f.wav("long.wav", 16000, 1, 160000);
    // Extend only this generated fixture as a sparse, long PCM source.
    let frames = 160_000_000u32; let data_bytes = frames * 2;
    let mut file = OpenOptions::new().write(true).open(&source).unwrap();
    file.seek(SeekFrom::Start(4)).unwrap(); file.write_all(&(36 + data_bytes).to_le_bytes()).unwrap();
    file.seek(SeekFrom::Start(40)).unwrap(); file.write_all(&data_bytes.to_le_bytes()).unwrap();
    file.set_len(44 + data_bytes as u64).unwrap(); drop(file);
    let stage = Stage::new(&f.spaces.ai_root).unwrap(); let output = stage.0.join("cancelled.wav");
    let cancel = AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        scope.spawn(|| { std::thread::sleep(Duration::from_millis(5)); cancel.store(true, Ordering::Release); });
        run(&locate_ffmpeg(&f.spaces).unwrap(), &source, &output, "wav", ConversionOptions::default(), &cancel, Instant::now() + Duration::from_secs(20), OUTPUT_LIMIT)
    });
    assert!(result.unwrap_err().contains("取消"));
    // On Windows an unreaped child retaining its output handle would prevent cleanup.
    drop(stage); f.no_stages();
}

#[test]
fn executable_resolution_rejects_workspace_and_relative_paths() {
    let f = Fixture::new();
    fs::write(f.spaces.user_root.join("ffmpeg.exe"), b"do not execute").unwrap();
    assert!(trusted_executable(&f.spaces, &f.spaces.user_root.join("ffmpeg.exe")).is_err());
    assert!(trusted_executable(&f.spaces, Path::new("ffmpeg.exe")).is_err());
}

#[test]
fn wave_validator_rejects_truncation_empty_data_and_invalid_frame_alignment() {
    let f = Fixture::new(); let source = f.wav("source.wav", 16000, 1, 100);
    let mut bytes = fs::read(&source).unwrap(); bytes.pop(); fs::write(&source, &bytes).unwrap();
    assert!(validate_wave(&source, ConversionOptions::default()).is_err());
    f.wav("empty.wav", 16000, 1, 0);
    assert!(validate_wave(&f.spaces.user_root.join("empty.wav"), ConversionOptions::default()).is_err());
}
