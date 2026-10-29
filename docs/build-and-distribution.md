# 构建与分发分析

2026-10-28：已生成 NSIS 当前用户安装器，支持选择目录、快捷方式及 WebView2 缺失时联网补装。数据保留策略与未执行的动态验证见[安装器说明](windows-installer.md)。便携 ZIP 继续保留，MSI 尚未制作。

2026-10-26：已完成主程序和后端 Release 构建，便携包含官方 VC Release CRT；ZIP 实测 37.62 MiB，解压 57.09 MiB。检查范围、产物及尚未验证的部分见 [Release 验证记录](release-validation.md)。

2026-10-25 更新：当前已改用固定源码的音频精简 FFmpeg，构建方法与源许可材料见[音频精简构建](ffmpeg-audio-build.md)。`package-desktop.ps1` 可整理内嵌前端的 Release 便携目录，`-LocalDevelopmentOnly` 则整理 Debug 体验目录；两者均核验并携带 FFmpeg 源码包。NSIS/MSI 安装器尚未启用。以下数值保留为替换前的测量记录，不代表新包体积。

## 音频精简版实测（2026-10-25）

| 项目 | 字节 | MiB |
| --- | ---: | ---: |
| FFmpeg/FFprobe、5 个 FFmpeg DLL、LAME、winpthreads | 7,912,497 | 7.55 |
| FFmpeg 完整目录（含匹配源码 ZIP、许可与构建记录） | 33,365,430 | 31.82 |
| 当前 Debug 体验目录（含主程序及 C++ 后端） | 69,594,326 | 66.37 |
| Debug 体验目录 ZIP | 41,341,789 | 39.43 |

测量产物：`build/AudioProcess-audio-development-20261025/` 及同名 ZIP。这是 Debug 体验包，不是 NSIS/MSI 安装器；没有混入整个 target 目录、PDB、旧 FFmpeg 备份或可选 ASR 插件。运行程序仍使用原路径 `apps/desktop/src-tauri/target/debug/audioprocess-prototype.exe`。

原全量包 FFmpeg 目录约 159.89 MiB，新版连源码及声明约 31.82 MiB。源码单独 ZIP 为 25,264,019 字节，已放入随附 FFmpeg 目录，无需另找 2 GiB 的上游缓存来陪同这个新构建分发。

已验证：211 个原有内置音频解码器保留、4 个输出编码器、15 个输入 fixture、16 种互转、中文路径及清空 PATH 运行；Rust 305 项通过（14 项显式 opt-in 忽略），另单独通过真实工具适配层→Workflow→C++ Graph→导出测试。桌面 Debug 构建和整包材料校验成功，全程未启动桌面窗口或调用在线模型。

## 旧全量包测量记录

测量日期：2026-10-24。当前是Windows x64本地开发版，不是已经完成许可审查的正式发布包。

## 构建与整理目录不是同一件事

| 用途 | 命令/流程 | 产物与依赖 |
| --- | --- | --- |
| 开发联调 | 在`apps/desktop`运行`npm run tauri -- dev` | 使用Vite开发服务器，适合改代码，不适合直接交给别人 |
| 本机独立体验 | `npm run tauri -- build --debug --no-bundle` | 前端嵌入`src-tauri/target/debug/audioprocess-prototype.exe`，不依赖Vite；仍需同目录C++后端和FFmpeg资源 |
| 整理开发副本 | 仓库根运行`pwsh -File scripts/package-desktop.ps1 -LocalDevelopmentOnly -Destination <新的目录>` | 复制已构建主程序、`control-cli.exe`、`ffmpeg/`及开发版说明，不重新编译、不生成安装器、不覆盖已有目录 |
| 未来正式分发 | C++ Release + Tauri Release + 配置NSIS/MSI打包 | 先补齐对应源码、NOTICE和许可审查，再启用正式发布流程；当前release和分发脚本有明确门禁 |

当前实际开发构建顺序（VS2022 x64开发环境，已安装项目依赖）：

```powershell
# 仓库根；首次配置/修改C++时执行
cmake --preset windows-msvc
cmake --build --preset debug --target control-cli --parallel

# 只改前端/Rust时，从这里开始即可
Set-Location apps/desktop
npm run tauri -- build --debug --no-bundle
```

CMake的POST_BUILD会准备`src-tauri/binaries`中的目标平台后端；Tauri将后端放到主程序旁边。FFmpeg需事先按`docs/ffmpeg-bundle.md`准备，构建本身不下载它。
编译输出位置并没有因为“本地开发版”而变化：`build/AudioProcess-local-development-*`只是之前另行整理的副本，旧副本不会随着新编译自动更新。

## 实测体积

从本轮最新嵌入前端Debug EXE整理出的完整开发副本测量；未运行程序、未复制个人配置/对话/音频/插件。
`MiB = 1,048,576字节`。

| 部分 | 字节 | MiB |
| --- | ---: | ---: |
| Tauri/Rust主程序（已内嵌前端） | 25,560,576 | 24.38 |
| C++后端`control-cli.exe`（当前Debug构建） | 10,662,912 | 10.17 |
| FFmpeg/FFprobe、7个DLL及现有说明材料 | 167,654,905 | 159.89 |
| 模式标记、开发版提示 | 819 | <0.01 |
| 完整运行目录 | 203,879,212 | **194.43** |
| ZIP，.NET Optimal/Deflate实压 | 86,068,701 | **82.08** |

ZIP索引复核22个条目、解压长度203,879,212字节，与目录总大小一致。
测量目录为`build/distribution-size-check-20261024`，ZIP为同名`.zip`；它们仅用于本机分析，不代表已解除分发门禁。

以下不包含在基础包中：

- WebView2 Runtime：使用目标Windows上已有的运行时，不把整个浏览器放入上述目录。
- 可选节点插件与模型：当前独立ASR base包实测约58.59 MiB，small包约183.13 MiB；二者均未混入本次基础包或提交范围。
- 用户API配置、会话、运行记录、音频，以及`target/`编译缓存、`node_modules/`、PDB等开发材料。
- 此前约2 GiB的FFmpeg依赖源码缓存：它不是运行库体积，也不是必须塞进主安装包的内容。正式分发时需提供相应材料，不能拿不匹配的最新源码替代。

## 建议的正式分发路线

1. 普通用户优先提供NSIS安装包，开发者另保留干净的便携目录。目标机不需要Node.js、Rust或Visual Studio，但需要兼容的Windows x64和WebView2。
2. 安装器检查WebView2，缺失时下载运行时；完全离线部署则单独准备包含WebView2的安装方案，会额外增加体积。不要把上述82.08 MiB当作未来安装包的精确大小；安装器压缩方式、Release产物和WebView2策略都会影响它。[Tauri官方Windows分发说明](https://v2.tauri.app/distribute/windows-installer/)
3. 许可材料齐备后，将C++和Rust都切换为Release并复测。当前`bundle.active=false`，仅运行不带`--debug`的命令也不是完整发布流程；Release实际大小尚未测量，不给未经验证的缩小承诺。
4. 首要瘦身对象是FFmpeg（当前约占82%）。当前通用构建含大量视频/字幕能力；应重新构建音频专用版本并同步整理源码/许可证，而不是直接删DLL。算法/ASR插件保持可选安装。

当前所选FFmpeg为LGPL shared版本，但完整匹配依赖源码与各依赖NOTICE仍不齐，正式release门禁继续保留。源码可作为独立配套下载，不必跟每份运行目录一起安装；仍需按实际组件及许可证核对提供方式。[FFmpeg官方许可检查清单](https://ffmpeg.org/legal.html)

这份记录是工程构建与体积分析，不是法律意见或“零许可/专利风险”承诺。
