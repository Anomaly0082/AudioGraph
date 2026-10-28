# Release 便携包验证（2026-10-26）

已先提交 `2d31581`（精简 FFmpeg 和稳定性改进）；旧 ASR 文件没有纳入。随后完成 Release 构建，并补充打包脚本的 VC 运行库部署和 Release sidecar 校验。后续脚本/验证记录改动尚未提交；本次不是干净工作树构建，也未推送远程。

## 产物

- 便携目录：`build/AudioProcess-release-20261026/`，启动 `AudioProcess.exe`。
- ZIP：`build/AudioProcess-release-20261026.zip`。
- 解压体积 **59,861,731 字节（57.09 MiB）**；ZIP **39,442,453 字节（37.62 MiB）**。
- 包内 `SHA256SUMS.txt` 核对文件；ZIP 旁的 `.sha256` 核对压缩包。`BUILD-AND-VALIDATION.json` 记录构建模式、主程序/后端 hash 和检查边界。
- 主程序与 C++ 后端均为 Release；FFmpeg 含对应源码及许可材料。可选 ASR、SDK 测试插件、PDB、开发工具均未放入交付包。
- 这是便携 ZIP，尚未制作 NSIS/MSI 安装器。需要 Windows x64 和 WebView2 Runtime；不需要安装开发环境或配置 FFmpeg。真实干净机器尚未验证。

## 解决的分发缺口

C++ Release 后端实际导入 `MSVCP140.dll`、`VCRUNTIME140.dll` 和 `VCRUNTIME140_1.dll`。现从已安装 VS 2022 的 `VC/Redist/MSVC/<version>/x64/Microsoft.VC143.CRT` 目录原样复制允许名单内的 Release DLL，记录版本及 SHA-256。拒绝 Debug 非分发目录、System32 来源、非 x64 目录、链接和覆盖现有文件。

该包采用应用本地部署；更新包时需维护这些 DLL。依据 [Microsoft 部署说明](https://learn.microsoft.com/en-us/cpp/windows/redistributing-visual-cpp-files?view=msvc-170)及 [VS 2022 可分发文件清单](https://learn.microsoft.com/en-us/visualstudio/releases/2022/redistribution)，许可条件仍适用。包内 `MICROSOFT-CRT-NOTICE.txt` 单独说明，不能把微软运行库视为 FFmpeg 的 LGPL 代码。

打包前后将旁置 `control-cli.exe` 与本次 `build/Release/control-cli.exe` 的 SHA-256 比对，避免 Debug 构建污染共用 Tauri sidecar 目录。主 EXE 使用自身内嵌信息校验 `release` 和已嵌入前端。

## 已执行检查

1. CMake `release` preset 构建 control-cli；Tauri `build --no-bundle` 构建 Release 主程序。
2. 打包前后校验主程序模式、C++ 后端一致性、FFmpeg 全量材料、微软 CRT 字节/哈希。
3. 将完整包复制到独立中文空格目录；检查 21 个包内 PE 的 x64 架构与普通/延迟导入依赖。所有非系统 DLL 在包内闭合。WebView2 的运行时发现不由此静态检查证明。
4. 子进程 PATH 仅包含 Windows System32，工作目录不在源码仓库根目录，测试使用私有生成数据：WAV/FLAC/MP3/M4A 转换与 ffprobe 元数据、后端文本任务、示例 Gain 插件发现及输出增益数值均通过。测试插件仅存在验证目录，不随应用打包。
5. 对搬迁后的 Release 后端执行 `tests/control_cli_tests.ps1`：非法请求、超长消息、任务生命周期、文件边界、设备权限拒绝和 EOF 退出通过。
6. Release 下 9 项 `session_state` 测试通过，覆盖状态/权限往返、损坏文件及无效草稿保存边界。
7. CRT 打包合成测试 8 项通过；独立子代理复核通过。

日志和详细结果位于 `build/release-*-*.log`、`build/release-validation-20261026/report.json`。打包与自动验收合计完成上述检查，单独的 Python 验证脚本不代替打包前后材料校验。

**没有启动 Tauri/WebView2 窗口，没有操作真实音频设备、用户配置或工作区，没有调用收费模型 API。** 这仍是同一台 Windows 主机上的搬迁测试，不是独立干净 Windows/VM 验证；窗口显示、WebView2 缺失时的实际体验和另一台机器的启动保留为后续验证。

## 重复构建与验收

在 VS 2022 Developer PowerShell x64 中：

```powershell
cmake --build --preset release --target control-cli --parallel 4
Set-Location apps/desktop
npm run tauri -- build --no-bundle
Set-Location ../..
pwsh -File scripts/package-desktop.ps1 -Destination 'C:/new-release-output'
python scripts/verify-release-package.py --package 'C:/new-release-output' --output 'C:/new-verification-output' --dumpbin (Get-Command dumpbin.exe).Source --plugin build/plugin-sdk-standalone/plugin-build/package/Release
```

输出目录必须不存在。插件参数可省略，省略时不会声称已检查插件加载。`package-desktop.ps1` 可显式传入 `-ReleaseSidecar` 与 `-VcRedistDirectory`，默认使用本项目 Release 后端和 VS 2022 的 Release CRT。
