# 音频精简版 FFmpeg

应用直接调用随附的 `ffmpeg/bin/ffmpeg.exe` 和 `ffprobe.exe`，无需用户安装 FFmpeg、设置 PATH 或联网下载组件。界面与 AI 共用原有转换入口，原文件不覆盖。

## 构建范围

固定 FFmpeg revision `5c4f5744cd60943125a78db692b728210102f255` 与 LAME SVN r6835。完整输入 hash、音频解码器与组件名单在 `scripts/ffmpeg-audio.lock.json`。

- 输入容器：WAV、MP3、M4A、AAC、FLAC、OGG/OGA、WebM；保留对应内置音频解码能力，包括 AAC、ALAC、Vorbis、Opus 与常见 PCM。
- 输出：PCM16 WAV、FLAC、MP3（LAME）、M4A（AAC）；保留采样率/声道转换。
- 关闭视频编码器、GPU、网络、自动发现外部库、avdevice、swscale；没有 rav1e 或其它 Rust 视频依赖。
- FFmpeg 与 LAME 采用共享库；LAME 解码器及命令行前端关闭。使用 FFmpeg 自带 MP3 解码器。
- 无 NASM 构建关闭 x86 汇编优化，使用本机 GCC 的普通优化；不宣称与原全量包性能一致。

## 开发者构建

仅开发者需要 Windows x64、PowerShell 7、Python 3.12+ 和 MSYS2 MinGW64。当前工具链为 GCC 15.2.0（MSYS2 revision 8）；make 4.4.1-3 是项目私有构建工具，不安装到系统、不随程序运行。实际编译器版本、命令、配置和 PE 导入记录保存在构建目录 `metadata/`。

```powershell
pwsh -NoProfile -File scripts/prepare-ffmpeg.ps1 -MsysRoot 'C:/tools/msys64' -Offline -Force
```

`-Jobs` 可设 1–4。`-CacheDirectory` 默认 `build/ffmpeg-cache`，`-BuildDirectory` 默认 `build/ffmpeg-audio`。输入或组件锁改变时选择新的构建目录。`-Force` 将现有资源目录改名保留，再安装新目录，不删除用户替换库。兼容旧的 `-RuntimeOnly` 参数，但该参数也会包含源码及许可材料。

缓存需要锁文件指定的 FFmpeg ZIP、LAME tar.xz 和 make 工具包。FFmpeg/LAME 原包保存在随附源码 ZIP 的 `sources/` 下，解压后复制到缓存即可；make 可按锁文件固定 URL 获取并校验 SHA-256，或复用已校验缓存。程序用户不需要做这些操作。

如只重建 FFmpeg 而不安装进应用资源目录，调用 `scripts/build-ffmpeg-audio.ps1`，参数相同（没有 `-Force`），产物为 `<BuildDirectory>/stage`。

## 源码与许可

每份完整 FFmpeg 目录都带有：

- `licenses/`：FFmpeg、LAME、MinGW、GCC Runtime Library Exception 和相关目标运行库声明原文。
- `NOTICE.txt`：组件、来源、替换权利和源码位置。
- `sources/ffmpeg-audio-sources.zip`：FFmpeg/LAME 原始源码包、构建脚本、源锁、真实构建配置/记录、声明及本说明。
- `bundle-provenance.json`：源版本、运行文件及分发文件的 SHA-256 清单。

使用已识别工具链从普通源码编译，保留 GCC Runtime Library Exception 原文；它针对适用的编译器运行库提供附加许可，不将 FFmpeg/LAME 的义务消除。FFmpeg 按本次实际构建的 LGPL-2.1-or-later 处理，LAME 按其 COPYING/LICENSE 处理。源码与二进制一起提供并保留声明；发布下载时也要保持二者在同一发布位置。

发行准备时检查文件完整性。运行时不校验固定 hash，不限制用户替换兼容的 FFmpeg/LAME 动态库。`redistribution_ready` 仅表示本 FFmpeg 组件的材料准备状态，不代表整个 AudioGraph 及全部插件已经完成许可审核。

这是一次从固定源码的新构建，不再依赖旧 BtbN 视频依赖或其历史 Cargo 缓存。旧全量构建核对报告作为历史记录保留。
