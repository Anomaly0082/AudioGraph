//! Independent tests. Fixtures are synthesized here; no user audio or network.
use super::*;
struct Fixture { root:PathBuf, spaces:ToolWorkspaces }
impl Fixture {
    fn new()->Self {
        let root=std::env::temp_dir().join(format!("audio-conversion-review-{}-{}",std::process::id(),SERIAL.fetch_add(1,Ordering::Relaxed)));
        fs::create_dir(&root).unwrap();let user_root=root.join("user");let ai_root=root.join("ai");
        fs::create_dir(&user_root).unwrap();fs::create_dir(&ai_root).unwrap();
        Self{root,spaces:ToolWorkspaces{user_root,ai_root}}
    }
    fn source(&self,name:&str,rate:u32,channels:u16)->Vec<u8>{
        let frames=rate/5;let data_size=frames*channels as u32*2;let mut bytes=Vec::new();
        bytes.extend_from_slice(b"RIFF");bytes.extend_from_slice(&(36+data_size).to_le_bytes());bytes.extend_from_slice(b"WAVEfmt ");bytes.extend_from_slice(&16u32.to_le_bytes());bytes.extend_from_slice(&1u16.to_le_bytes());bytes.extend_from_slice(&channels.to_le_bytes());bytes.extend_from_slice(&rate.to_le_bytes());bytes.extend_from_slice(&(rate*channels as u32*2).to_le_bytes());bytes.extend_from_slice(&(channels*2).to_le_bytes());bytes.extend_from_slice(&16u16.to_le_bytes());bytes.extend_from_slice(b"data");bytes.extend_from_slice(&data_size.to_le_bytes());
        for i in 0..frames*channels as u32 {bytes.extend_from_slice(&(((i%100) as i16-50)*100).to_le_bytes());}
        fs::write(self.spaces.user_root.join(name),&bytes).unwrap();bytes
    }
    fn run(&self,source:&str,target_space:&str,target:&str,options:ConversionOptions)->Result<Value,String>{
        convert(&self.spaces,"user",source,target_space,target,options,&AtomicBool::new(false),Instant::now()+Duration::from_secs(15))
    }
    fn clean(&self){for root in [&self.spaces.user_root,&self.spaces.ai_root]{assert!(!fs::read_dir(root).unwrap().any(|e|e.unwrap().file_name().to_string_lossy().starts_with(".audio-convert-")),"conversion staging leaked");}}
}
impl Drop for Fixture{fn drop(&mut self){let _=fs::remove_dir_all(&self.root);}}

fn output_options(format:&str)->ConversionOptions {
    serde_json::from_value(json!({"output_format":format,"sample_rate":22050,"channels":1})).unwrap()
}

#[test]
fn review_four_output_formats_roundtrip_without_source_changes_or_overwrites(){
    use sha2::{Digest,Sha256};
    let f=Fixture::new();let original=f.source("原始 音频.wav",22050,1);
    let source_hash=Sha256::digest(&original);
    for format in ["wav","flac","mp3","m4a"]{
        let name=format!("generated.{format}");f.run("原始 音频.wav","ai",&name,output_options(format)).unwrap();
        let generated=fs::read(f.spaces.ai_root.join(&name)).unwrap();let hash=Sha256::digest(&generated);
        for output_format in ["wav","flac","mp3","m4a"]{
            let destination=format!("roundtrip-{format}.{output_format}");
            let converted=convert(&f.spaces,"ai",&name,"ai",&destination,output_options(output_format),&AtomicBool::new(false),Instant::now()+Duration::from_secs(15)).unwrap();
            if output_format=="wav"{assert!(converted["frames"].as_u64().unwrap()>1000,"{format} to {output_format} lost audio");}
            else{assert!(converted["duration_seconds"].as_f64().unwrap()>0.05,"{format} to {output_format} lost audio");}
            assert_eq!(Sha256::digest(fs::read(f.spaces.ai_root.join(&name)).unwrap()),hash,"source changed during {format} to {output_format}");
            let published=fs::read(f.spaces.ai_root.join(&destination)).unwrap();
            assert!(convert(&f.spaces,"ai",&name,"ai",&destination,output_options(output_format),&AtomicBool::new(false),Instant::now()+Duration::from_secs(15)).is_err());
            assert_eq!(fs::read(f.spaces.ai_root.join(&destination)).unwrap(),published,"existing {destination} overwritten");
            if format=="flac"&&output_format=="wav"{assert_eq!(published,original,"lossless FLAC roundtrip changed samples/header");}
        }
    }
    assert_eq!(Sha256::digest(fs::read(f.spaces.user_root.join("原始 音频.wav")).unwrap()),source_hash);f.clean();
}

#[test]
fn review_output_formats_reject_unknown_and_mismatched_extensions(){
    let f=Fixture::new();f.source("source.wav",22050,1);
    for format in ["exe","aac","WAV;-i","../mp3",""]{
        if let Ok(options)=serde_json::from_value::<ConversionOptions>(json!({"output_format":format})){
            assert!(f.run("source.wav","ai","out.wav",options).is_err(),"accepted {format}");
        }
    }
    for(format,name)in [("mp3","mislabel.wav"),("flac","stream:ads.flac"),("m4a","../escape.m4a"),("wav","NUL.wav")]{assert!(f.run("source.wav","ai",name,output_options(format)).is_err(),"{format} {name}");}
    assert_eq!(fs::read_dir(&f.spaces.ai_root).unwrap().count(),0);f.clean();
}

#[test]
fn review_clean_path_relocated_bundle_child(){
    if std::env::var_os("AUDIOGRAPH_RELOCATED_REVIEW").is_none(){return;}
    let f=Fixture::new();let original=f.source("原始 中文 音频.wav",22050,1);
    assert_eq!(std::env::var("PATH").unwrap(),"");assert!(std::env::var_os("AUDIOGRAPH_FFMPEG_PATH").is_none());
    let executable=locate_ffmpeg(&f.spaces).unwrap();let own=std::env::current_exe().unwrap();
    let expected=fs::canonicalize(own.parent().unwrap().join("ffmpeg/bin/ffmpeg.exe")).unwrap();assert_eq!(executable,expected,"did not use relocated runtime bundle");
    for format in ["wav","flac","mp3","m4a"]{f.run("原始 中文 音频.wav","ai",&format!("relocated.{format}"),output_options(format)).unwrap();}
    assert_eq!(fs::read(f.spaces.user_root.join("原始 中文 音频.wav")).unwrap(),original);f.clean();
}

#[cfg(windows)]
#[test]
fn review_clean_path_relocated_chinese_bundle_works_without_machine_ffmpeg(){
    let f=Fixture::new();let executable=locate_ffmpeg(&f.spaces).unwrap();
    let manifest:Value=serde_json::from_slice(&fs::read(executable.parent().unwrap().parent().unwrap().join("bundle-provenance.json")).expect("only copy a prepared bundled runtime, never the machine's GPL-full FFmpeg")).unwrap();
    assert_eq!(manifest["license"],"LGPL-2.1-or-later");assert_eq!(manifest["variant"],"windows-x64-audio-lgpl");
    assert_eq!(manifest["redistribution_ready"],true);
    let package=f.root.join("中文 空格 搬移后的开发版");fs::create_dir(&package).unwrap();let bin=package.join("ffmpeg/bin");fs::create_dir_all(&bin).unwrap();
    // Copy only the actual shipped runtime, never discover another executable via PATH.
    let source_bin=executable.parent().unwrap();for item in fs::read_dir(source_bin).unwrap(){let item=item.unwrap();let name=item.file_name();let ext=item.path().extension().map(|v|v.to_string_lossy().to_ascii_lowercase());if matches!(ext.as_deref(),Some("exe"|"dll")){fs::copy(item.path(),bin.join(name)).unwrap();}}
    let test_executable=package.join("conversion-review.exe");fs::copy(std::env::current_exe().unwrap(),&test_executable).unwrap();
    let result=Command::new(&test_executable).args(["--exact","audio_conversion::review_tests::review_clean_path_relocated_bundle_child","--nocapture"]).env("AUDIOGRAPH_RELOCATED_REVIEW","1").env("PATH","").env_remove("AUDIOGRAPH_FFMPEG_PATH").current_dir(&package).stdin(Stdio::null()).output().unwrap();
    assert!(result.status.success(),"relocated child failed: {}\n{}",String::from_utf8_lossy(&result.stdout),String::from_utf8_lossy(&result.stderr));
}

#[test]
fn review_conversion_precancel_deadline_invalid_options_and_paths_do_not_publish(){
    let f=Fixture::new();let original=f.source("source.wav",22050,1);
    assert!(convert(&f.spaces,"user","source.wav","user","cancelled.wav",ConversionOptions::default(),&AtomicBool::new(true),Instant::now()+Duration::from_secs(1)).is_err());
    assert!(convert(&f.spaces,"user","source.wav","user","expired.wav",ConversionOptions::default(),&AtomicBool::new(false),Instant::now()).is_err());
    for name in ["../escape.wav","stream:ads.wav","NUL.wav","","bad.mp3"]{assert!(f.run("source.wav","user",name,ConversionOptions::default()).is_err(),"{name}");}
    for options in [ConversionOptions{sample_rate:Some(0),channels:None,..Default::default()},ConversionOptions{sample_rate:None,channels:Some(6),..Default::default()}]{assert!(f.run("source.wav","ai","invalid.wav",options).is_err());}
    assert_eq!(fs::read(f.spaces.user_root.join("source.wav")).unwrap(),original);assert_eq!(fs::read_dir(&f.spaces.user_root).unwrap().count(),1);assert_eq!(fs::read_dir(&f.spaces.ai_root).unwrap().count(),0);f.clean();
}

#[test]
fn review_conversion_hardlinks_existing_targets_and_deep_ai_rejected(){
    let f=Fixture::new();let original=f.source("source.wav",22050,1);fs::write(f.spaces.user_root.join("keep.wav"),b"existing").unwrap();
    assert!(f.run("source.wav","user","source.wav",ConversionOptions::default()).is_err());assert!(f.run("source.wav","user","keep.wav",ConversionOptions::default()).is_err());
    fs::hard_link(f.spaces.user_root.join("source.wav"),f.spaces.user_root.join("linked.wav")).unwrap();assert!(f.run("linked.wav","user","blocked.wav",ConversionOptions::default()).is_err());
    fs::create_dir_all(f.spaces.ai_root.join("a/b/c/d")).unwrap();assert!(f.run("source.wav","ai","a/b/c/d/out.wav",ConversionOptions::default()).is_err());
    assert_eq!(fs::read(f.spaces.user_root.join("keep.wav")).unwrap(),b"existing");assert_eq!(fs::read(f.spaces.user_root.join("source.wav")).unwrap(),original);assert!(!f.spaces.user_root.join("blocked.wav").exists());f.clean();
}

#[test]
fn review_real_conversion_preserves_samples_and_classic_tag_at_high_rate(){
    let f=Fixture::new();let original=f.source("中文 空格-录音.wav",96000,2);
    let result=f.run("中文 空格-录音.wav","ai","out.wav",ConversionOptions::default()).unwrap();
    let output=fs::read(f.spaces.ai_root.join("out.wav")).unwrap();assert_eq!(u16::from_le_bytes(output[20..22].try_into().unwrap()),1,"graph reader cannot read extensible WAV");assert_eq!(output,original,"lossless WAV conversion changed PCM samples/rate/channels");assert_eq!(result["sample_rate"],96000);assert_eq!(result["channels"],2);assert_eq!(fs::read(f.spaces.user_root.join("中文 空格-录音.wav")).unwrap(),original);f.clean();
}

#[test]
fn review_real_corrupt_and_playlist_inputs_fail_without_output(){
    let f=Fixture::new();
    for(name,bytes)in [("corrupt.mp3",b"not an mp3".as_slice()),("playlist.mp3",b"#EXTM3U\nhttp://127.0.0.1:1/private\n".as_slice()),("concat.wav",b"ffconcat version 1.0\nfile 'C:/private/file.wav'\n".as_slice())]{fs::write(f.spaces.user_root.join(name),bytes).unwrap();assert!(f.run(name,"ai","out.wav",ConversionOptions::default()).is_err(),"{name}");assert!(!f.spaces.ai_root.join("out.wav").exists());f.clean();}
}

#[test]
fn review_real_all_supported_containers_produce_pcm16_and_preserve_source(){
    let f = Fixture::new();
    let inputs = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio-input");
    for name in ["aac.aac", "aac.m4a", "alac.m4a", "flac.flac", "mp3.mp3", "opus.ogg", "opus.webm", "pcm-f32.wav", "pcm-f64.wav", "pcm-s16.wav", "pcm-s24.wav", "pcm-s32.wav", "pcm-u8.wav", "vorbis.ogg", "vorbis.webm"] {
        let name = name.to_string();
        let extension = name.replace(".", "-");
        let encoded = f.spaces.user_root.join(&name);
        fs::copy(inputs.join(&name), &encoded).unwrap();
        let source = name;
        let original=fs::read(f.spaces.user_root.join(&source)).unwrap();let target=format!("{extension}.wav");let result=f.run(&source,"ai",&target,ConversionOptions{sample_rate:Some(16000),channels:Some(1),..Default::default()}).unwrap();let wav=fs::read(f.spaces.ai_root.join(&target)).unwrap();assert_eq!(&wav[..4],b"RIFF");assert_eq!(&wav[8..12],b"WAVE");assert_eq!(u16::from_le_bytes(wav[20..22].try_into().unwrap()),1);assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()),16);assert_eq!(result["sample_rate"],16000);assert_eq!(result["channels"],1);assert!(result["frames"].as_u64().unwrap()>1000);assert_eq!(fs::read(f.spaces.user_root.join(&source)).unwrap(),original);f.clean();
    }
}

#[test]
fn review_running_decoder_cancels_before_publication_and_reaps_staging(){
    use std::sync::Arc;
    let f=Fixture::new();let mut bytes=f.source("long.wav",16000,1);
    let data_size=32u32*1024*1024;bytes.truncate(44);bytes[4..8].copy_from_slice(&(36+data_size).to_le_bytes());bytes[40..44].copy_from_slice(&data_size.to_le_bytes());
    let source=f.spaces.user_root.join("long.wav");fs::write(&source,&bytes).unwrap();OpenOptions::new().write(true).open(&source).unwrap().set_len(44+data_size as u64).unwrap();
    let flag=Arc::new(AtomicBool::new(false));let child_flag=flag.clone();let spaces=f.spaces.clone();
    let worker=std::thread::spawn(move||convert(&spaces,"user","long.wav","ai","cancelled.wav",ConversionOptions{sample_rate:Some(192000),channels:Some(2),..Default::default()},&child_flag,Instant::now()+Duration::from_secs(15)));
    let until=Instant::now()+Duration::from_secs(10);let mut decoder_started=false;
    while Instant::now()<until&&!worker.is_finished(){
        decoder_started=fs::read_dir(&f.spaces.ai_root).unwrap().filter_map(Result::ok).any(|entry|entry.file_name().to_string_lossy().starts_with(".audio-convert-")&&entry.path().join("output.wav").exists());
        if decoder_started{break;}std::thread::sleep(Duration::from_millis(1));
    }
    flag.store(true,Ordering::Release);let result=worker.join().unwrap();
    assert!(decoder_started,"decoder output was never observed before cancellation");assert!(result.unwrap_err().contains("取消"));
    assert!(!f.spaces.ai_root.join("cancelled.wav").exists());assert_eq!(fs::metadata(source).unwrap().len(),44+data_size as u64);f.clean();
}
