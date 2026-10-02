# AudioProcess

面向 AI 可控音频处理的 C++20 实验工程。当前包含整段离线 DAG、同步离线分块图，以及 Windows 最小实时线性 Graph；三者复用 Graph 描述，但使用不同执行契约。

## 已实现

- 已注册节点：wav_input、gain、peak_meter、wav_output、text_input、text_output。
- 离线语音降噪：rnnoise_denoise 封装固定 RNNoise 默认模型，48 kHz 单声道 Audio → Audio；本地 CPU，无需模型 API，保持帧数。使用方法见 [降噪说明](docs/denoise.md)。
- 离线格式适配：audio_downmix_mono 将双声道平均转单声道，audio_resample 复用 miniaudio 重采样；支持8–192 kHz单/双声道。audio.inspect 查询工作区WAV格式，内置AI可先获取真实元数据再生成提案。见 [格式适配与手动验收](docs/audio-format.md)。
- 正式流式节点：wav_stream_input、stream_gain、wav_stream_output。相同 Graph JSON 结构，独立 AudioStream 端口和流式工厂。
- Graph JSON v1：节点、参数、连接、exports；相对路径基于配置文件目录。
- 参数描述：类型、必填、默认值、范围、单位和文本枚举。
- 独立图校验：不调用节点工厂，不打开音频或写产物。
- 执行器：拓扑排序、每任务新节点、只读共享音频、类型与结果有效性检查、协作式取消。
- 文件输出：原子独占创建，拒绝覆盖已有文件；中文路径和 UTF-8 文本。
- 分块 Executor：逐级格式准备、连续帧校验、零到多块输出、按上游到下游排尾和协作取消。正式支持离线分块，尚未接麦克风。
- 实验契约：旧流式 vector 输出和异步 future 方案仍保留为实验，不等同于正式分块或实时能力。
- 实时 Graph：realtime_input、realtime_gain、realtime_output 注册到同一 Registry；控制线程准备线性计划，设备回调执行预分配处理器。有缓冲、时钟漂移补偿、静音探测和统计，不录音。
- 受控任务入口：常驻 control-cli 通过 JSON Lines 查询能力、校验图、异步提交、查询状态、取消和读取结果；单活动任务，有宿主文件/设备权限限制。
- 最小 MCP 适配：官方 SDK 的本地 stdio 服务，开放离线/分块任务工具；标准 MCP 客户端链路已测试，实际 AI 客户端仍需显式挂载配置。
- 桌面工作台：Tauri/React 通过 Rust 常驻连接 control-cli，提供可视化节点/连线编辑、参数侧栏、Graph JSON、模板、校验、任务与结果。
- 内置 AI Graph 助手：自填 OpenAI 兼容地址、模型和内存 Key，生成离线/分块提案，经本地校验和人工确认后执行，再解释真实结果；无需配置外部 MCP 客户端。
- 最小参数对比：固定离线Graph与WAV副本，手填或AI提出2～4组参数，确认后串行运行、保存结果与人工评价，支持重启读取和恢复参数。见 [参数实验](docs/parameter-experiments.md)。

## 构建与测试

使用 VS2022 Developer PowerShell。JSON、miniaudio和RNNoise代码已固定；RNNoise权重因许可待上游澄清，不随源码提交。第一次克隆需阅读 [模型准备说明](third_party/rnnoise/README.integration.md)，显式准备本地模型；本工作目录已准备。准备后C++构建不联网。

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

## 最小实时 Graph（Windows）

```powershell
.\build\Debug\realtime-cli.exe --list-devices
.\build\Debug\realtime-cli.exe --describe-node realtime_gain
# 把枚举结果中的输入/输出 ID 填到 graph-gain.json 的端点 device_id 后：
.\build\Debug\realtime-cli.exe --graph .\examples\realtime\graph-gain.json --validate
.\build\Debug\realtime-cli.exe --graph .\examples\realtime\graph-gain.json --probe --seconds 10
```

默认静音探测；显式添加 `--monitor` 才实际输出输入音频，试听请先戴耳机并调低音量，避免啸叫。`graph-passthrough.json` 示例去掉 Gain，展示同协议配置不同节点链。旧 `--config session.json` 保留为转换到 Graph 的兼容入口。使用方法、线程边界和验收限制见 [实时 Graph](docs/realtime-audio.md)。

## 受控任务接口

```powershell
.\build\Debug\control-cli.exe --workspace C:\AAAProject\AudioProcess
```

保持进程连接，逐行发送结构化 JSON 请求，例如 `{"schema_version":1,"id":"q1","op":"capabilities"}`。整段、离线分块和实时图复用同一任务生命周期；宿主默认不允许设备访问。协议、状态与安全边界见 [受控任务接口](docs/control-api.md)。桌面及 MCP 适配层均复用此接口。

## MCP 接入

`apps/mcp` 是独立 Node.js 22+ 适配层，使用固定版本官方 SDK，不依赖桌面窗口。先构建 C++ control-cli，再在 apps/mcp 中运行 `npm ci --ignore-scripts`、`npm test`。启动路径和客户端配置见 [MCP 使用说明](docs/mcp.md)。

第一版只开放 offline/streaming，不开放设备或任意脚本。推荐授权独立音频数据目录；配置示例不会自动修改全局 AI 客户端设置。MCP 与桌面各自拥有自己的后台会话，不共享 task_id。

## 目录与阅读顺序

```text
include/audioprocess/   C++ 数据结构、Node、Graph 与 Executor 接口
src/                   核心和文件节点实现
apps/graph_demo/       JSON/命令行协议入口
apps/audio_cli/        保留 M0 AudioBlock 旁路实验
apps/realtime_cli/     Windows 实时设备会话入口
apps/control_cli/      常驻 JSON Lines 任务控制入口
apps/mcp/              官方 SDK stdio MCP 适配与独立测试
apps/desktop/          React + Rust/Tauri 最小控制台
tests/                 核心、文件、配置、CLI 和契约实验测试
examples/graphs/       可编辑 Graph 配置
examples/realtime/     实时 Graph 与旧会话配置示例
schemas/               文档结构 Schema
third_party/           固定版本 nlohmann/json、miniaudio 和许可证
```

建议读 `graph.h → node.h → graph_validator.cpp → sync_graph_executor.cpp → prototype_nodes.cpp`。分块路径另读 `streaming_node.h → streaming_graph_executor.cpp → streaming_nodes.cpp`。JSON 解析位于独立 `audio_graph_io` 库，节点接口无需了解 JSON 库。

实时路径：`realtime_node.h → realtime_graph_executor.cpp → realtime_nodes.cpp → realtime_bridge.cpp → realtime_session.cpp`。DSP、调度、设备分别阅读，不需要先研究 miniaudio 的实现。

## 桌面控制台

桌面使用control-cli任务接口，主导航为处理工作台、编辑器、文件、运行记录、设置。编辑器内的Graph页签保留节点画布，Workflow页签支持JSON打开/另存、校验、运行与停止，不需要模型服务。右上角是唯一工作区入口，在工作台选择输入并用AI或模板准备方案；编辑器可添加/删除节点、拖动位置、连接端口、修改参数和导出，保留JSON高级编辑。文件页只读浏览用户/AI工作区，支持文本预览、本地音频试听和配置载入，不自动执行或上传。运行记录统一保存手动/AI Graph与Workflow的快照和结果，支持父子记录及文件指纹检查；旧参数实验已退出正式界面。实时设备需明确授权，默认静音。使用流程见 [桌面说明](docs/desktop.md)，记录边界见[运行记录](docs/run-records.md)。

也可使用 AI Graph 助手，用自然语言提出文件处理需求。可解析但校验失败的提案或本助手执行失败的任务，可以手动请求一次修正，再检查差异、重新确认执行。需要支持 Chat Completions 工具调用的模型服务；地址、模型和Key可保存到本机用户配置目录的明文JSON，启动自动恢复，不进入仓库。操作步骤、数据发送范围及兼容限制见 [内置 AI 说明](docs/embedded-ai.md)。

```powershell
cmake --build --preset debug --target control-cli
Set-Location .\apps\desktop
npm ci
npm test
npm run tauri -- dev
```

Windows x64 CMake 构建会复制 control-cli Sidecar 到 `src-tauri/binaries`；重新运行 Tauri 开发/构建流程才会把新 Sidecar 放到桌面程序旁。仅复制 C++ 产物不会自动更新已运行的桌面后台进程。旧 graph-demo 命令仍保留，但桌面不再调用它。

## 当前边界

### 实验性节点插件（Windows x64）

`sdk/plugin/include/audiograph/plugin.h` 是独立的 C ABI 0.1 头文件，覆盖整段同步节点的描述、能力查询、创建、运行和销毁。普通参数用具名、带类型的 C 值视图传递，不要求插件引入 JSON 解析器。插件内部仍可用 C++ 和自己的算法库；输入只读借用，输出通过同步回调复制到宿主内存。ABI 0.x 尚未冻结，不是沙箱，不承诺任意工具链兼容。

`examples/plugins/standalone/` 是只依赖安装后 SDK 的外部项目：自有辅助库、增益节点和明确标记为模拟的音频转文本节点，没有真实 ASR、HTTP 或 GPU 实现。`tests/plugin_sdk/` 仍是独立C11/C++测试宿主；正式接入现在由 `src/plugin_host.cpp` 完成，通过现有注册表和同步Executor运行，不把插件算法编译进主程序。

在 VS2022 Developer PowerShell、Windows x64 下，从仓库根目录运行：

```powershell
cmake -S . -B build -G "Visual Studio 17 2022" -A x64
ctest --test-dir build -C Debug -R plugin-sdk-standalone --output-on-failure
```

该测试会在 `build/plugin-sdk-standalone/` 安装SDK，将示例和测试源码复制到独立目录，再分别配置、编译和通过真实DLL边界调用。示例只拿到安装的头文件与CMake包，不链接 `audio_core`，不请求模型服务、不读取用户音频。生成的实验插件包位于其 `plugin-build/package/Debug/`，包含DLL、节点描述和带SHA256的manifest。正式宿主回归另用 `ctest --test-dir build -C Debug -R plugin-host-tests --output-on-failure`，其前置fixture会自动构建示例包。

SDK也可单独安装后由第三方项目消费：`cmake -S sdk/plugin -B build/sdk`，再执行 `cmake --install build/sdk --prefix <独立SDK目录>`；第三方CMake使用 `find_package(AudioGraphPluginSDK 0.1.0 EXACT CONFIG REQUIRED)` 并链接头文件接口目标 `AudioGraph::PluginSDK`。取消是协作的；原生崩溃隔离、动态文件端口策略、统一资源/凭据服务和图形安装器仍是后续阶段。

使用方法：关闭软件，把完整示例包复制为本机用户配置目录下 `com.audioprocess.prototype/plugins/example/`（通常在 `%APPDATA%`）。不要把三个文件直接散放在plugins根目录，不要只复制DLL。重启后打开工作区，在设置页查看实际插件目录、可用包和错误；编辑器与AI节点目录自动出现 `org.audiograph.example.gain_v1`、`org.audiograph.example.mock_asr_v1`。可用内置WAV输入/输出连接插件Gain，按普通Graph设置gain_db运行。软件不会自动安装或启用下载来的包；目录内容属于用户明确安装的可信原生代码。

软件首次启动会自动创建设置页显示的空插件目录（必要时补齐父目录），但不会自动安装插件，也不会为无插件状态创建运行快照或插件运行数据。目录被同名文件占用、存在链接或不可写时会明确报错，不覆盖或替换已有文件。

本轮宿主边界：

可参考 [plugin-gain.json](examples/graphs/plugin-gain.json)，将内容载入已启用插件的桌面Graph编辑器，在当前工作区准备 `input.wav`；运行会新建 `plugin-gain-output.wav`，重复运行需改输出名称。旧 `graph-demo` 仍只有内置注册表，不用于这个插件示例。

- 应用启动时一次捕获最多16个直接子包的manifest指纹；用户和AI后台共享同一快照，重连不重扫。安装、更新或移除后需要重启。缺manifest、链接目录、扫描超限等快照捕获错误会拒绝连接并给出目录；可读取但内容无效的单包由C++报告诊断，保留内置节点。
- 静态发现/Graph校验不加载DLL，首次执行才核验ABI、身份及描述并加载；已加载入口/manifest/nodes的文件句柄保持到对应后台进程退出。重复plugin_id/type冲突涉及的包全部拒绝，不按扫描顺序选版本。SHA256用于变化检测，不是签名或可信认证。
- 当前受理入口DLL+nodes.json两个被manifest指纹固定的文件；复杂依赖/资源清单尚未开放，额外资源或插件自有联网状态不属于已验证的包指纹范围。包内算法可像示例一样静态链接私有库；不承诺任意第三方DLL依赖组合隔离。
- 支持整段同步Audio/Text/Number端口及Number/Text/Boolean参数；明确拒绝外部FilePath声明、流式/实时/异步能力。文件读写继续用内置节点组合。插件仍能自行调用本机库或HTTP；原生代码不受AI文件工具权限沙箱约束，不能保证其不上传音频。
- 为避免单次节点发现撑满AI上下文，完整节点目录（含内置）暂限32KiB；超额包明确拒绝，不静默截断参数。目录分页与更多类型可后续扩展。单次插件输入/音频输出各限256MiB，所有Text输出合计限64KiB，无Audio输出的节点总输出预算也为64KiB；超限明确失败，不把大文本塞断结果通道。调用协作期限60秒，不承诺硬抢占；失控插件可能使当前后台进程中断。
- 节点目录及运行配置中的node_plugins保存插件ID、实现版本和manifest指纹；不会把私有目录或凭据当作节点元数据发送给AI。安装列表变化、插件缺失或执行失败不会删除原Graph文件，也不声称自动迁移配置。历史版本自动比对、依赖锁定及缺失节点的完整占位编辑体验仍待后续接入。

2026-10-02：独立SDK用例在Windows x64 VS2022的Debug与Release均通过，覆盖C11/C++函数表调用、不同CRT下的数据复制、失败/取消与UTF-8边界、CRLF源码描述一致性和包指纹；另有5项现有核心回归通过，完成独立代码审查。

### 音频与执行能力

- 统一工具助手：工作台现有入口内提供Graph/Workflow两种权限模式；双工作区文件工具、有限多轮调用和AI空间Graph运行已接入。支持受限[Workflow JSON程序](docs/workflow-v1.md)的校验与执行，见[工具与空间说明](docs/agent-tools.md)。

- SyncGraphExecutor 的 Audio 是完整 AudioClip；长录音可能占用较多内存。StreamingGraphExecutor 的 AudioStream 使用借用块，不缓存完整录音；格式转换接口已预留，但没有重采样算法。
- 文件编解码仅 PCM16 WAV；离线 Graph 已有显式重采样/转单声道节点（8–192 kHz，单/双声道），不支持其他布局或高保真sinc。RNNoise降噪本体限48 kHz单声道整段语音，未接实时设备。未实现 MP3/FLAC、ASR、TTS、GPU 或云端音频节点。
- 实时图限线性、48 kHz 单声道、同格式同帧数处理器，不支持分支、变长输出、录音、热改图或运行中调参；设备拔插、真人试听及端到端延迟仍需人工验收，不承诺硬实时。
- 正式分块执行仍是同步离线，不支持流式分支、混合整段/流式图或异步任务。M0 旁路工具保留，正式流式 Graph 不通过它调度。
- 已有受控任务协议、Tauri桌面、离线MCP适配和内置AI工具助手。已有受限离线参数实验及Workflow v1（变量、有限循环、条件和白名单工具调用）；尚无自动搜索/评分算法或长程恢复机制，AI不能执行任意Python。
- 取消是协作请求，失败可能留下部分新文件，图不提供文件事务回滚。
- 需求与设计目录为本地讨论材料，按用户要求不提交 Git。
