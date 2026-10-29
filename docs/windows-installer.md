# Windows 安装器（2026-10-28）

用户已在本机验证安装和卸载正常；升级及缺少 WebView2 的干净机器场景仍未实测。下文的自动检查不替代这些实际生命周期验证。

产物：`build/AudioGraph-0.1.0-x64-setup.exe`。使用 Tauri CLI 2.11.4 / NSIS 3.11，生成当前用户安装器；现有 Release 便携 ZIP 继续保留。安装器版本取自应用配置，本次为 0.1.0。

## 安装行为

- 可选择安装目录，默认当前用户 LocalAppData 下的 AudioGraph。
- 创建开始菜单快捷方式，完成页可选择桌面快捷方式；安装完成后启动程序默认不勾选。
- 在 Windows 的应用列表注册卸载项，安装目录也有 `uninstall.exe`。
- 沿用应用 identifier `com.audioprocess.prototype`，不迁移或重置现有 API 配置、会话、插件和工作区状态。
- 检测当前用户的 AudioGraph 主进程和 `control-cli.exe`；存在运行任务时提示关闭并退出，静默模式返回非零。不会自动终止任务。检查按进程名，可能同时阻止在另一个目录运行的同名便携版。

## WebView2

内嵌约 1.8 MiB 的微软官方 Evergreen bootstrapper；构建时验证有效 Microsoft Corporation 签名，记录 SHA-256，解包时再次核对字节。检测机器和当前用户注册项，空值或 `0.0.0.0` 视为缺失；升级也保留此检测。

**已有运行时通常直接跳过，缺失时仍需联网下载运行时。** 这是内嵌引导程序，不是离线完整运行时。补装失败会中止。它不会在构建/检查安装包时被执行，也不会被本次工具调用安装到用户电脑。

## 数据保留策略

基于官方模板做了以下收窄：

1. 移除“删除应用数据”复选框及删除 AppData 的代码。
2. 卸载仅逐个删除清单中程序自有文件，目录使用非递归 `RMDir`；安装目录内额外放置的工作区/音频/笔记不会因递归清理被删除。
3. 移除旧 WiX 安装枚举迁移，避免调用同名产品的卸载器；不根据注册表中的任意旧 exe 名称删除文件。
4. 保留标准 NSIS 修复/升级流程，既有配置和插件目录在应用数据区，不列入安装/卸载载荷。

这些是**已审查的脚本策略**，并非已在真实 Windows 安装、升级、卸载中完成验证。程序自有文件仍会在更新时替换、卸载时删除，不应把用户内容覆盖到这些已占用的程序文件名上。

## 构建

在已有 VS 2022 x64 开发环境、前端依赖和音频精简 FFmpeg 材料的基础上：

```powershell
pwsh -NoProfile -File scripts/build-desktop-installer.ps1
```

默认先构建 Release 后端/主程序。`-SkipBuild` 可复用已验证的 Release；`-Destination` 指定新的安装器路径，目标已存在时拒绝覆盖。工具缓存仅在项目 `target/.tauri/`，不安装 NSIS 到系统。

脚本先复用便携包准备流程核对主程序、Release sidecar、微软 CRT、FFmpeg 源码许可，再交给 Tauri NSIS 打包。自定义模板与 CLI 版本绑定，升级 CLI 前需重新审查。基础 `tauri.conf.json` 的便携构建设置保持不变，安装专用覆盖在 `tauri.installer.conf.json`。

Tauri 会临时将主 EXE 的 bundle-kind 标记从 `UNK` 改为 `NSS` 并还原源文件。校验器从可信源 EXE 预先计算仅这一处三字节变化后的哈希，拒绝其它变化；没有放宽整文件核对。

## 自动验证与边界

- NSIS 编译成功，已解包并核对 **50 个载荷文件**与 WebView2 bootstrapper 的哈希。
- 核对生成的 NSIS 源脚本及构建记录；卸载清单为 50 个程序载荷加 `uninstall.exe`，合计 **51 项**。
- 6 项纯策略测试覆盖递归删除、用户目录删除、任意文件删除、终止进程、升级跳过 WebView2 和路径越界的拒绝；6 项标记预测测试通过。
- 内存删除模型验证额外工作区/音频/笔记哨兵不属于卸载目标，不代表真正执行过 Windows 卸载。
- 解包后的主程序仍标识内嵌前端的 Release，FFmpeg 分发材料清单再次验证通过；独立子代理审核通过。
- 未执行安装器、卸载器、WebView2 安装或桌面程序，未写入用户安装注册项/快捷方式。真实安装页外观、首次安装、升级、卸载及缺 WebView2 的联网补装保留给隔离 VM 验证。

验证命令（只读取/解包，不安装）：

```powershell
python scripts/test-desktop-installer.py
pwsh -File scripts/test-nsis-build-info.ps1
python scripts/verify-desktop-installer.py --installer build/AudioGraph-0.1.0-x64-setup.exe --output build/new-installer-inspection --sevenzip 'C:/Program Files/7-Zip/7z.exe'
```

构建记录为安装器旁的 `.build.json`，验证报告位于 `build/installer-delivery-verification-20261028/report.json`。安装器自身未进行代码签名；微软 bootstrapper 的签名不代表整个 AudioGraph 安装器已签名。

## 上游来源和许可

自定义模板改自 [Tauri CLI 2.11.4 的 installer.nsi](https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.11.4/crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi)，改动在文件头声明。采用 zlib 压缩。包内 `installer-licenses/` 保留 Tauri MIT、NSIS COPYING、nsis-tauri-utils 0.5.3 MIT 原文；这些不替代 FFmpeg、微软运行库和应用其它组件各自的声明。
