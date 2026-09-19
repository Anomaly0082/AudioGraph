# AudioProcess

一个面向 AI Agent 的可扩展音频处理平台实验工程。核心目标是提供能力可发现、操作可验证、功能可扩展的音频处理接口；ASR、自动优化、实时设备和 MCP 都作为扩展接入。

当前 M0 版本只实现最小闭环：

```text
PCM16 WAV → AudioBlock → BypassNode → PCM16 WAV
```

后续将依次增加节点注册表、参数 Schema、可序列化 Graph、控制接口以及 Evaluator/Optimizer 扩展。

## P0 GraphExecutor + Tauri 原型

当前还包含一个带类型端口的同步 DAG 原型：

```text
WAV Input → Gain → WAV Output
                 ↘ Peak Meter → Number
```

它验证：

- `NodeRegistry` 能力发现。
- 端口类型、必要输入和环路检查。
- 拓扑排序与同步节点调度。
- Tauri/React → Rust → C++ Sidecar 调用链。

详细说明见 `需求与设计/06-P0-GraphExecutor与Tauri原型.md`。

下一阶段设计见 [P1：可编程节点图设计计划](需求与设计/07-P1-可编程节点图设计计划.md)。该计划尚未实现，目标是通过 JSON 配置已有节点，由 C++ 校验和调度，并提供通用的桌面调用入口。

## 构建

在 Visual Studio 2022 Developer PowerShell 中运行：

```powershell
cmake -S . -B build -G "Visual Studio 17 2022" -A x64
cmake --build build --config Debug
ctest --test-dir build -C Debug --output-on-failure
```

## 使用

```powershell
.\build\Debug\audio-cli.exe `
  --input .\input.wav `
  --output .\output.wav `
  --block-size 256
```

当前文件输入仅支持 RIFF/WAVE PCM16 单声道或多声道文件，输出为 PCM16 WAV。

## Tauri 开发模式

先构建 `graph-demo`，再启动桌面端：

```powershell
cmake --build build --config Debug --target graph-demo

Set-Location .\apps\desktop
npm install
npm run tauri -- dev
```
