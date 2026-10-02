# AudioGraph Plugin SDK 0.1 开发包

本说明面向解压后的独立开发包根目录。只需此包即可构建 Windows x64 插件，不需要 AudioGraph 核心源码、Node.js 或 Rust。ABI 0.1 是实验接口，不承诺稳定的第三方二进制兼容性。

准备 Visual Studio 2022 的“使用 C++ 的桌面开发”工作负载及 Windows SDK，以及 CMake 3.25 或更高版本。示例和测试使用 C++20；公开 ABI 头可供 C11/C++17 使用。

```text
README.md
include/audiograph/plugin.h
lib/cmake/AudioGraphPluginSDK/
docs/plugin-development.md
docs/plugin-packages.md
examples/standalone/
examples/graphs/plugin-gain.json
tests/sdk-host/
scripts/verify-example.cmake
```

在开发包根目录打开 PowerShell，先验证原样示例。WORK_ROOT 指向一个新的绝对路径，放在 SDK 目录之外：

```powershell
$kit = (Get-Location).Path
cmake -DWORK_ROOT="C:/Temp/AudioGraph-example-check" -P .\scripts\verify-example.cmake
```

脚本仅使用包内已安装的 SDK，复制并构建示例和测试。WORK_ROOT 必须尚不存在，重复验证请选择另一个新目录。默认 `CONFIG=Debug`、`GENERATOR=Visual Studio 17 2022`、`PLATFORM=x64`；可通过 `-DCONFIG=Release` 改用 Release。`tests/sdk-host` 硬编码示例节点 ID 和 Gain/Mock ASR 行为，是示例回归测试，不是任意插件的通用验证器。

手工构建示例：

```powershell
$kit = (Get-Location).Path
cmake -S .\examples\standalone -B C:/AudioGraphWork/my-plugin/build -G "Visual Studio 17 2022" -A x64 "-DCMAKE_PREFIX_PATH=$kit"
cmake --build C:/AudioGraphWork/my-plugin/build --config Debug
```

`$kit` 是当前开发包的绝对路径。生成的可安装插件目录是 `C:/AudioGraphWork/my-plugin/build/package/Debug`；SDK 根目录不是插件包。开发自己的节点时，先复制 `examples/standalone` 到自己的工程目录，然后修改该副本。

阅读[节点开发约定](docs/plugin-development.md)和[插件包规范](docs/plugin-packages.md)。`examples/graphs/plugin-gain.json` 是配套示例 Graph；替换节点身份后也要调整 Graph 中的类型和参数。独立回归测试通过后，仍需在实际 AudioGraph 中安装、重启并验证发现、执行、取消及错误处理。

节点开发约定还说明了宿主如何补齐默认参数，以及如何通过 Graph `exports` 取得 Number 输出；数值导出不需要转换成文件。本开发包文档修订于 2026-10-02（r2），SDK/CMake 包版本及 C ABI 仍为 0.1.0/0.1，没有接口变更。

可将下面的任务交给另一个 AI：

> 只使用提供的 AudioGraph Plugin SDK 开发包，先阅读 README 和 docs/plugin-development.md、docs/plugin-packages.md，再复制 examples/standalone 实现所需节点。只包含公开 SDK 头，通过 AudioGraph::PluginSDK 构建；不要依赖核心源码或修改宿主。保持 ABI 常量和现有执行模型，同步更新节点、入口 API 和 manifest 身份，生成带 SHA256 的插件包。先运行原样示例回归，再为新算法验证输入、输出、取消、容量和错误。遇到能力缺口请报告，不要绕过权限或宿主限制。交付源码、构建命令、插件包和测试结果，并列出仍需实际安装验证的项目。
