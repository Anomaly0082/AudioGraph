# 本地自带 FFmpeg 开发版

当前固定使用 BtbN `autobuild-2026-10-22-13-06` 的
`ffmpeg-n8.1.3-23-g5c4f5744cd-win64-lgpl-shared-8.1.zip`。
实际运行 `-L` 确认为 **LGPL-3.0-or-later**；`-buildconf` 确认 shared、未启用 GPL/nonfree。
此处没有使用本机 PATH 上的 GPL full_build。

当前用户选择暂不获取完整依赖源码，只保留本地开发版。
`bundle-provenance.json` 的 `redistribution_ready` 保持 `false`。
标准 release 构建及分发脚本会拒绝继续；这不代表能阻止用户自行复制 debug 程序。

## 准备与本地运行

```powershell
.\scripts\prepare-ffmpeg.ps1 -RuntimeOnly
# 已有完整校验缓存时可加 -Offline
# 已有资源时，-Force 会将旧资源移到相邻备份目录，保留用户替换版本。

cd apps\desktop\src-tauri
cargo test --offline audio_conversion
```

脚本只从固定 URL 获取并按 `scripts/ffmpeg-bundle.lock.json` 校验 SHA-256。
生成的 `resources/ffmpeg/bin` 含 `ffmpeg.exe`、`ffprobe.exe` 与七个 FFmpeg DLL；
`licenses`、NOTICE、实际构建配置、来源元数据一并保留。下载缓存和运行二进制不入版本库。
Cargo debug 构建将资源复制到 exe 相邻的 `ffmpeg` 目录；Tauri resources 也使用这一结构。
运行时不回退到 PATH、当前目录或环境中的 FFmpeg；缺失时明确报错。

明确需要搬移本机开发副本时：

```powershell
# 在 apps/desktop 下先生成内嵌前端的 debug 程序，不需要本地 Vite 服务器。
npm run tauri -- build --debug --no-bundle
# 回到仓库根目录后：
.\scripts\package-desktop.ps1 -LocalDevelopmentOnly -Destination 'C:\本地开发测试\音频 工具'
```

目标必须不存在，脚本不会覆盖已有目录。脚本要求成功的 Tauri CLI 内嵌前端构建标记，
不接受普通 `cargo build` 的程序；`control-cli.exe` 缺失时也会在创建目录前拒绝继续。
这不是正式可分发便携包。运行时不校验固定 hash，用户可替换兼容的 FFmpeg/LGPL DLL。

## 尚缺的分发材料

已缓存精确 FFmpeg revision 与 BtbN 构建脚本压缩包；但完整依赖源码仍未取得。
官方匹配该次构建的 `download-cache` Actions ZIP 大小为 2,210,375,345 字节。
连接器限制单资产 512 MiB，匿名下载返回 401；本轮没有分段绕过限制、获取凭据或下载此大件。

未来若决定继续，使用已授权的 GitHub 登录下载：

[匹配的完整依赖源码 artifact](https://github.com/BtbN/FFmpeg-Builds/actions/runs/39776938430/artifacts/12424698518)

将原始 ZIP 放在 `build/ffmpeg-cache/download-cache.zip`，预期 SHA-256 为
`f89c6c43969d92c8b95cb459a90fbafbc743d3d3cc4d16c610317f9ceef922f8`。
该 artifact 公布的失效时间为 2026-11-05；如失效，不能用最新依赖源码冒充对应版本。
取得缓存之后仍需核对实际依赖版本、补全各依赖许可证/NOTICE、构建脚本与 patch，
并将真实源码包与二进制包一起分发；仅有 URL 或 FFmpeg 本身源码不够。
目前准备脚本不会自动宣告完成此审查。

许可方案以 [FFmpeg 官方说明](https://ffmpeg.org/legal.html) 和实际构建结果为依据。
这些措施不是法律意见，也不代表不存在编解码器专利或地区相关的法律风险。

## 收尾验证（2026-10-24）

- Rust 常规回归 281 项通过；付费 API、真实插件等显式 opt-in 测试不计入此数字。
- 前端 195 项通过，生产前端构建成功。无界面交互测试覆盖四种格式、扩展名校验、编码失败、取消、切页/切工作区和重名拒绝；其原生调用边界使用 mock，不冒充桌面真人验收。
- 真实 FFmpeg 测试覆盖 WAV/FLAC/MP3/M4A 的 16 种互转组合、原文件字节不变、禁止覆盖、损坏输入、取消/超时暂存清理，以及搬移到中文空格目录、清空 PATH 后仍能转换。
- 显式运行真实 AI 工具适配层与 Workflow：M4A → PCM16 WAV → audio_inspect → C++ Graph → 文件导出成功；不是在线大模型自主编排测试。
- 新增独立 opt-in 测试：四种格式分别 → audio_convert → 16 kHz 单声道 WAV → 真实 CPU ASR 插件 → TXT 导出。使用固定哈希的公共短语音、临时工作区与临时插件安装目录；全部得到中文文本，文本端口与两处 TXT 字节一致，源文件未改。仅验证兼容性，不评估识别准确率。
- 独立代码审核未发现阻断收尾的问题。32 MiB 试听上限未改；正式分发仍受上文源码材料缺口限制。

在 VS Developer PowerShell 中复测端到端链路（无需设备、UI 或模型 API）：

```powershell
cargo test --offline --manifest-path apps/desktop/src-tauri/Cargo.toml audio_conversion_workflow_to_graph_roundtrip -- --ignored --nocapture

# 可选：需要已经构建的独立可信 ASR 插件和已有公共测试样本，不自动下载。
$env:AUDIOPROCESS_ASR_PACKAGE = "$PWD/build/asr-whisper-avx2/package/Release"
$env:AUDIOPROCESS_ASR_WAV = "$PWD/build/asr-deps/zh.wav"
cargo test --offline --manifest-path apps/desktop/src-tauri/Cargo.toml audio_conversion_four_formats_to_real_asr_workflow -- --ignored --nocapture
```

第二项测试在 `audio_conversion_asr_tests.rs`；普通测试不要求安装 ASR。
AVX2 包仅适用于支持该指令集的机器，也可显式选择兼容的非 AVX2 包。
