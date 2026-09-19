# AudioProcess

面向 AI 可控音频处理的 C++20 实验工程。当前交付 Graph 及以下的离线核心，包含 JSON 配置、节点注册、参数校验、同步 DAG 执行和文件输入输出。

## 已实现

- 已注册节点：wav_input、gain、peak_meter、wav_output、text_input、text_output。
- Graph JSON v1：节点、参数、连接、exports；相对路径基于配置文件目录。
- 参数描述：类型、必填、默认值、范围、单位和文本枚举。
- 独立图校验：不调用节点工厂，不打开音频或写产物。
- 执行器：拓扑排序、每任务新节点、只读共享音频、类型与结果有效性检查、协作式取消。
- 文件输出：原子独占创建，拒绝覆盖已有文件；中文路径和 UTF-8 文本。
- 实验契约：流式 push/finish/reset、异步拥有型输入和协作取消。尚未接入正式 Graph。

## 构建与测试

使用 VS2022 Developer PowerShell，已固定 JSON 依赖源码，C++ 构建无需下载依赖。

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

## 目录与阅读顺序

```text
include/audioprocess/   C++ 数据结构、Node、Graph 与 Executor 接口
src/                   核心和文件节点实现
apps/graph_demo/       JSON/命令行协议入口
apps/audio_cli/        保留 M0 AudioBlock 旁路实验
apps/desktop/          现有 React + Rust/Tauri 演示
tests/                 核心、文件、配置、CLI 和契约实验测试
examples/graphs/       可编辑 Graph 配置
schemas/               文档结构 Schema
third_party/           固定版本 nlohmann/json 和许可证
```

建议读 `graph.h → node.h → graph_validator.cpp → sync_graph_executor.cpp → prototype_nodes.cpp`。JSON 解析位于独立 `audio_graph_io` 库，节点接口无需了解 JSON 库。

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

- 正式 Executor 仅支持同步离线 DAG，Audio 是完整 AudioClip；长录音可能占用较多内存。
- 文件编解码仅 PCM16 WAV，未实现 MP3/FLAC、采样率转换、实时设备、ASR、TTS、GPU 或云端服务。
- 实验异步/流式接口不是实时产品能力；M0 AudioBlock 处理链与通用 Graph 尚未统一。
- 上层受控 Workflow、AI 接口与参数搜索尚未实现。AI 编排不以执行任意 Python 为前提。
- 取消是协作请求，失败可能留下部分新文件，图不提供文件事务回滚。
- 需求与设计目录为本地讨论材料，按用户要求不提交 Git。
