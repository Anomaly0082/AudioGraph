//! Test-only HTTP bridge to exercise the real range reader with a headless browser.
//! Production uses a private Tauri protocol and never starts this listener.
use super::*;
use std::{io::Write, net::TcpListener};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

#[test]
#[ignore = "Driven by workspace-audio-playback-smoke.mjs with an existing headless Chromium"]
fn headless_range_playback_bridge() {
    assert_eq!(std::env::var("AUDIOGRAPH_AUDIO_BRIDGE").as_deref(), Ok("1"));
    let fixture = Fixture(std::env::temp_dir().join(format!("audio-range-browser-{}", random_token().unwrap())));
    let user = fixture.0.join("用户音频");
    let ai = fixture.0.join("ai");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::create_dir(&ai).unwrap();
    // 240 seconds, 48 kHz stereo PCM16: 43.9 MiB, with sparse silent samples.
    let frames = 48000u32 * 240;
    let data_bytes = frames * 4;
    let mut header = Vec::new();
    header.extend(b"RIFF"); header.extend((36 + data_bytes).to_le_bytes());
    header.extend(b"WAVEfmt "); header.extend(16u32.to_le_bytes());
    header.extend(1u16.to_le_bytes()); header.extend(2u16.to_le_bytes());
    header.extend(48000u32.to_le_bytes()); header.extend(192000u32.to_le_bytes());
    header.extend(4u16.to_le_bytes()); header.extend(16u16.to_le_bytes());
    header.extend(b"data"); header.extend(data_bytes.to_le_bytes());
    let name = "大文件 试听.wav";
    let mut file = File::create(user.join(name)).unwrap();
    file.write_all(&header).unwrap(); file.set_len(44 + u64::from(data_bytes)).unwrap(); drop(file);
    let mut audio = open_stream("main".into(), "main".into(), "fixture".into(), user.clone(),
        ToolWorkspaces { user_root: user, ai_root: ai }, "user".into(), name.into()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let route = format!("/{}", random_token().unwrap());
    let stop = format!("/{}", random_token().unwrap());
    println!("AUDIO_PREVIEW_READY {}", serde_json::json!({
        "url":format!("http://{address}{route}"), "stop":format!("http://{address}{stop}"), "bytes":audio.bytes
    }));
    std::io::stdout().flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(50);
    let mut max_body = 0usize;
    let mut max_offset = 0u64;
    let mut requests = 0usize;
    let mut stopped = false;
    while Instant::now() < deadline {
        let (mut socket, _) = match listener.accept() {
            Ok(pair) => pair,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5)); continue;
            },
            Err(error) => panic!("{error}"),
        };
        socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        socket.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut raw = Vec::new();
        while raw.len() < 16384 && !raw.ends_with(b"\r\n\r\n") {
            let mut byte = [0u8];
            match socket.read(&mut byte) { Ok(1) => raw.push(byte[0]), _ => break }
        }
        let raw = String::from_utf8(raw).unwrap();
        let mut lines = raw.split("\r\n");
        let mut first = lines.next().unwrap_or("").split_whitespace();
        let method = first.next().unwrap_or("GET");
        let path = first.next().unwrap_or("/");
        if path == stop {
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            stopped = true; break;
        }
        let mut builder = Request::builder().method(method).uri(format!("http://{address}{path}"));
        for line in lines.filter(|line| !line.is_empty()) {
            if let Some((name, value)) = line.split_once(':') { builder = builder.header(name, value.trim()); }
        }
        let request = builder.body(Vec::new()).unwrap();
        let response = if path == route { serve(&mut audio, &request).unwrap() } else { empty(404) };
        if path == route {
            requests += 1;
            max_body = max_body.max(response.body().len());
            if let Some(range) = response.headers().get("content-range").and_then(|s| s.to_str().ok()) {
                max_offset = max_offset.max(range.strip_prefix("bytes ").unwrap().split('-').next().unwrap().parse().unwrap());
            }
        }
        let mut wire = format!("HTTP/1.1 {} OK\r\nConnection: close\r\n", response.status().as_u16());
        for (name, value) in response.headers() { wire.push_str(&format!("{}: {}\r\n", name, value.to_str().unwrap())); }
        if !response.headers().contains_key("content-length") { wire.push_str("Content-Length: 0\r\n"); }
        wire.push_str("\r\n");
        // Browsers may cancel a speculative range when seeking; a closed socket is expected.
        let _ = socket.write_all(wire.as_bytes()).and_then(|_| socket.write_all(response.body()));
    }
    println!("AUDIO_PREVIEW_STATS {}", serde_json::json!({"requests":requests,"max_body":max_body,"max_offset":max_offset}));
    assert!(stopped, "Browser harness did not finish before deadline");
    assert!(requests >= 2 && max_body > 0 && max_body as u64 <= CHUNK);
    assert!(max_offset > 32 * 1024 * 1024, "Seek must read beyond the former preview cap");
    drop(audio);
}
