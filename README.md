# AudioProcess

面向 AI 可控音频处理的 C++20 实验工程。当前包含配置驱动的离线 Graph，以及 Windows 实时设备桥接原型；实时路径暂未接入通用 Graph。

## 已实现

- 已注册节点：wav_input、gain、peak_meter、wav_output、text_input、text_output。
- 正式流式节点：wav_stream_input、stream_gain、wav_stream_output。相同 Graph JSON 结构，独立 AudioStream 端口和流式工厂。
- Graph JSON v1：节点、参数、连接、exports；相对路径基于配置文件目录。
- 参数描述：类型、必填、默认值、范围、单位和文本枚举。
- 独立图校验：不调用节点工厂，不打开音频或写产物。
- 执行器：拓扑排序、每任务新节点、只读共享音频、类型与结果有效性检查、协作式取消。
- 文件输出：原子独占创建，拒绝覆盖已有文件；中文路径和 UTF-8 文本。
- 分块 Executor：逐级格式准备、连续帧校验、零到多块输出、按上游到下游排尾和协作取消。正式支持离线分块，尚未接麦克风。
- 实验契约：旧流式 vector 输出和异步 future 方案仍保留为实验，不等同于正式分块或实时能力。
- 实时设备原型：显式选择 WASAPI 输入/输出，固定 Input → Gain → Output；有缓冲、时钟漂移补偿、静音探测和统计，不录音。

## 构建与测试

使用 VS2022 Developer PowerShell，已固定 JSON 和 miniaudio 依赖源码，C++ 构建无需下载依赖。

```powershell
cmake --preset windows-msvc
cmake --build --preset debug --parallel
ctest --preset debug
```

Release 对应 `cmake --build --preset release --parallel`、`ctest --preset release`。
也可使用原命令 `cmake -S . -B build -G "Visual Studio 17 2022" -A x64`。

## 配置驱动运行

```powershell
.\build\Debug\graph-demo.exe --list-nodes
.\build\Debug\graph-demo.exe --describe-node gain
.\build\Debug\graph-demo.exe --graph .\examples\graphs\text.json --validate
.\build\Debug\graph-demo.exe --graph .\examples\graphs\text.json
```

最后一条会创建 `examples/graphs/message.txt`。再次运行会拒绝覆盖；修改配置中的输出文件名即可继续实验。

三个音频示例为 passthrough.json、gain.json、gain-peak.json。先把 PCM16 WAV 放在 `examples/graphs/input.wav`，或修改各 JSON 的输入路径。输出也相对于 JSON 所在目录，不能覆盖输入或已有文件。

协议与扩展说明见 [Graph API](docs/graph-api.md)，格式见 [JSON Schema](schemas/graph-v1.schema.json)。

## 离线分块执行

```powershell
.\build\Debug\graph-demo.exe --graph .\examples\graphs\stream-gain.json --validate
.\build\Debug\graph-demo.exe --graph .\examples\graphs\stream-gain.json --block-size 256
```

准备 `examples/graphs/input.wav` 或修改配置中的路径。`stream-gain.json` 用三个正式流式节点执行 `WAV → Gain → WAV`，不把完整录音载入内存。结果包含输出路径、总帧数和削波采样数；输出已存在时拒绝覆盖。

块长是执行选项，默认 256 帧，范围 1～65536。详情见 [同步离线分块 Graph](docs/streaming-graph.md)。流式首版只支持单源、单输出的线性链，整段 DAG 原有分支能力不受影响。

## 实时设备实验（Windows）

```powershell
.\build\Debug\realtime-cli.exe --list-devices
# 把枚举结果中的输入/输出 ID 填到 examples/realtime/session.json 后：
.\build\Debug\realtime-cli.exe --config .\examples\realtime\session.json --validate
.\build\Debug\realtime-cli.exe --config .\examples\realtime\session.json --probe --seconds 10
```

默认静音探测；显式添加 `--monitor` 才实际输出输入音频，试听请先戴耳机并调低音量，避免啸叫。此配置是固定设备会话，不是 Graph JSON。使用方法、线程边界和验收限制见 [实时设备桥接](docs/realtime-audio.md)。

## 目录与阅读顺序

```text
include/audioprocess/   C++ 数据结构、Node、Graph 与 Executor 接口
src/                   核心和文件节点实现
apps/graph_demo/       JSON/命令行协议入口
apps/audio_cli/        保留 M0 AudioBlock 旁路实验
apps/realtime_cli/     Windows 实时设备会话入口
apps/desktop/          现有 React + Rust/Tauri 演示
tests/                 核心、文件、配置、CLI 和契约实验测试
examples/graphs/       可编辑 Graph 配置
examples/realtime/     固定实时设备会话配置
schemas/               文档结构 Schema
third_party/           固定版本 nlohmann/json、miniaudio 和许可证
```

建议读 `graph.h → node.h → graph_validator.cpp → sync_graph_executor.cpp → prototype_nodes.cpp`。分块路径另读 `streaming_node.h → streaming_graph_executor.cpp → streaming_nodes.cpp`。JSON 解析位于独立 `audio_graph_io` 库，节点接口无需了解 JSON 库。

## 桌面兼容

本阶段保留原 Tauri 固定图演示，不增加完整 JSON 编辑 UI。`graph-demo --input ... --output ... --gain-db ...` 保持原成功返回字段；错误同时输出机器 JSON 和 stderr 文本，兼容现有 Rust 解析。

```powershell
cmake --build --preset debug --target graph-demo
Set-Location .\apps\desktop
npm ci
npm run tauri -- dev
```

Windows x64 CMake 构建会复制 Sidecar 到 `src-tauri/binaries`；重新运行 Tauri 开发/构建流程才会把新 Sidecar 放到桌面程序旁。仅复制 C++ 源目录的产物不会自动更新已经构建的桌面 EXE。

## 当前边界

- SyncGraphExecutor 的 Audio 是完整 AudioClip；长录音可能占用较多内存。StreamingGraphExecutor 的 AudioStream 使用借用块，不缓存完整录音；格式转换接口已预留，但没有重采样算法。
- 文件编解码仅 PCM16 WAV；离线 Graph 未实现采样率转换。实时会话由 miniaudio 适配设备格式，但不等于 Graph 已有转换节点。未实现 MP3/FLAC、ASR、TTS、GPU 或云端服务。
- 实时会话是独立固定链，不支持任意实时 Graph、录音、热改图或桌面控制；设备拔插、真人试听及端到端延迟仍需人工验收，不承诺硬实时。
- 正式分块执行仍是同步离线，不支持流式分支、混合整段/流式图或异步任务。M0 旁路工具保留，正式流式 Graph 不通过它调度。
- 上层受控 Workflow、AI 接口与参数搜索尚未实现。AI 编排不以执行任意 Python 为前提。
- 取消是协作请求，失败可能留下部分新文件，图不提供文件事务回滚。
- 需求与设计目录为本地讨论材料，按用户要求不提交 Git。
