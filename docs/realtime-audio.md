# Windows 最小实时 Graph

P4 将 P3 的设备实验接回 Graph 主线：GraphDefinition → Registry → 纯校验/准备计划 → RealtimeGraphExecutor → 处理器。RealtimeSession 管理设备，RealtimeBridge 管理跨时钟缓冲，不再内置 Gain。原整段和离线分块 Node 契约保留。

## 使用

在项目目录构建后运行：

```powershell
.\build\Debug\realtime-cli.exe --list-devices
```

从 `inputs` 和 `outputs` 中分别复制完整 `id` 到 `examples/realtime/graph-gain.json` 中输入/输出节点的 `device_id`。必须选择明确端点；设备名称、列表下标不能代替 ID。不会修改系统默认音频设备，也不自动回退到其他端点。

```powershell
.\build\Debug\realtime-cli.exe --list-nodes
.\build\Debug\realtime-cli.exe --describe-node realtime_gain
.\build\Debug\realtime-cli.exe --graph .\examples\realtime\graph-gain.json --validate
.\build\Debug\realtime-cli.exe --graph .\examples\realtime\graph-gain.json --probe --seconds 10
# 仅在戴好耳机、调低输出音量后试听；不要与 --probe 同时传入：
.\build\Debug\realtime-cli.exe --graph .\examples\realtime\graph-gain.json --monitor --seconds 10
```

默认是 probe：采集和处理照常进行，但送到设备的音频强制为零。不会保存音频文件。只有命令行 `--monitor` 能开启实际输出，配置文件不能开启。时长默认 10 秒，范围 1～3600 秒；Ctrl+C 请求停止，设备 stop/uninit 由控制线程完成。

`--graph` 可附加 `--target-frames 960 --period-frames 256`，不能混用旧 `--config`、`--input`、`--output`、`--gain-db`。旧命令/会话 JSON 仍可读取，但仅转换成 Input→Gain→Output 的 Graph，随后走相同执行器。`--validate` 不创建节点或访问设备，成功只表示配置合法，不代表 ID 当前可用。

节点目录包含已注册的离线和实时节点，实时入口只接受符合实时契约的节点；根据 `execution_domain`、`realtime_role` 和 `realtime_capabilities` 选择，不能只看端口名字。

VB-CABLE 中，通常应把本程序输出指向播放端 `CABLE Input`，其他应用从录音端 `CABLE Output` 读取。名称易混淆，以设备枚举方向为准；若输出列表没有对应端点，本程序不会自行创建或启用设备。

## Graph 与兼容配置约束

Graph 使用现有 v1 的 nodes/connections/exports 结构；节点数组顺序不决定执行顺序。只接受一条完整链：一个 Source、零到多个 Processor、一个 Sink，最多 128 节点。exports 必须为空或省略；实时流不通过离线结果映射导出。

内置类型为 realtime_input（必填 Text device_id，输出 audio）、realtime_gain（gain_db 默认 0、范围 -24～12 dB，输入/输出 audio）、realtime_output（必填 Text device_id，输入 audio）。端点只有绑定描述，不在回调创建节点或调用设备接口。

`realtime_capabilities` 描述固定格式、最大块长、可变块长支持和 `offline_drivable`。这些是节点契约与校验依据，不是库能自动证明任意第三方代码实时安全。当前仅接受 48 kHz、单声道、float32 交错、保持帧数的处理器。文件供块复用处理器不等于把设备端点当作文件节点运行。

以下表格仅针对旧 `--config session.json` 兼容格式，不能套在 Graph 顶层：

| JSON 字段 | 规则 |
| --- | --- |
| schema_version | 必填整数 1 |
| input_device / output_device | 必填非空 UTF-8 ID，不允许 NUL |
| gain_db | 默认 0，有限数，范围 -24～12 dB |
| capacity_frames | 默认 4096，4～1048576 的 2 次幂 |
| target_frames | 默认 960，范围 2～capacity_frames-2 |
| period_frames | 默认 256，范围 32～2048；是请求值，不保证设备采用 |

旧配置拒绝未知字段、重复键、超过 64 KiB 或 8 层的配置；Graph 按既有协议限 4 MiB/64 层，并做线性图/执行契约校验。内部单声道输出复制为双声道；miniaudio 负责原生格式/采样率/声道适配。当前不是保留立体声的通用处理链。

## C++ 边界与线程

- `graph_codec.*`：复用 Graph JSON 与节点发现协议；`realtime_config.*` 只负责旧会话配置兼容读取。
- `realtime_node.h` / `realtime_nodes.cpp`：实时处理器接口与内置 Gain。process 原地处理借用 span，返回状态码；不能保留视图、分配、等待或抛异常。
- `realtime_graph_executor.*`：纯校验和编译、控制线程 prepare、音频线程逐节点 process。prepare 创建新实例，重复准备等价于重新开始处理任务，不继承历史。
- `realtime_session.*`：控制线程枚举、初始化、启动和停止 WASAPI 设备；miniaudio 隐藏在 PImpl 后面。
- `realtime_bridge.*`：设备无关的桥接核心；采集线程写入，播放线程读取、漂移补偿后分片调用已准备的计划。环形队列/工作块在启动前分配，回调不访问注册表、文件、日志或等待下游。
- `apps/realtime_cli/main.cpp`：参数、安全模式、停止轮询和机器可读结果。不属于 Tauri 前端；本阶段桌面仍使用原离线 sidecar。

Bridge 是单生产者/单消费者（SPSC），capture/render 各仅允许一个调用线程。Session 持有执行计划，Bridge 借用；必须在设备回调全部停止后才能销毁。Session 公共方法由同一控制线程串行调用。参数在 prepare 前确定，本阶段不提供运行中参数更新；P3 的 Bridge 私有 gain/setter 已移除，避免两套 Gain 调度。

启动先输出静音，累积目标水位后播放。队列满时丢弃新输入中放不下的后缀；欠载时有效前缀经过计划，其余输出静音并重新预缓冲；没有输入期间不推进处理器状态。独立设备时钟通过水位反馈与线性插值做 ±0.5% 的轻微速率补偿，不是高品质音乐重采样算法。最终设备输出限制在 [-1,1]，采集非有限值净化并计数。

Session 默认以不超过 256 帧的块调用计划，与设备原生回调周期独立。节点出错或产生非有限值时，回调只发布整数状态/节点索引并静音，后续不再调用失败计划；控制线程构造含节点位置的错误并停止。process 的 noexcept 只是契约，恶意或不守约的 C++ 节点不被沙箱隔离，也没有强制抢占能力。

显式选定的设备停止、路由改变或中断时，回调只设置故障标记，控制线程随后停止两端；不会自动切换默认设备。正常结束和初始化中途失败均清理已创建的设备。不保证阻塞的系统设备 API 能在固定时间内返回。

## 返回和指标

stdout 输出一个最终 JSON，stderr 每秒输出 metrics JSON；启动失败另有可读错误。帮助输出除外。`success` 仅表示没有已检测故障且两个方向都收到回调，不保证有语音、不保证无削波或满足延迟预算。失败退出码非零。

关键指标：`capture_frames`、`render_frames`、`dropped_frames`、`underflow_frames`、`buffering_silence_frames`、`clipped_samples`、`capture_peak`、`output_peak` 和 `resample_ratio`。`software_queue_latency_ms` 只计算软件队列，不含设备缓冲/系统/算法整体延迟，不能当作嘴到耳的延迟。各字段是近似快照，不保证同一时刻。

受限环境可能允许设备枚举但阻止初始化，返回通用 `Invalid argument (-2)`。若发生，可在正常用户终端用相同 `--probe` 命令对照；不能把所有 -2 都解释为权限问题，也不应因此默认要求管理员运行。

## P3 历史验证与未覆盖项

2026-09-20：MSVC Debug/Release 各 13/13 CTest 通过。新增测试覆盖有界缓冲、欠载、静音、Gain、非有限输入、并发生产消费、模拟时钟漂移及配置/CLI 拒绝行为。回调测试检查 C++ new/new[] 分配，不等于拦截了系统或第三方库的全部内存分配。

沙箱外静音实测：WO Mic → Realtek 3 秒，采集 145440 帧、输出 147840 帧；CABLE Output → Realtek 20 秒，采集 960480 帧、输出 962400 帧。两次均无队列丢帧/欠载，均有 2400 帧预缓冲静音。设备原生均 48 kHz，周期 480 帧；CABLE 原生双声道，经适配进入单声道核心。输入峰值为零，因此只证明回调及静音链路，不证明语音保真。

尚未验收：真人有声试听、真实异采样率设备、拔插故障、Ctrl+C 实机行为、长时间稳定性、端到端延迟、虚拟播放端输出。本机未枚举到 CABLE Input 播放端。自动测试不会打开麦克风，也不会播放音频。

## P4 验证结果

2026-09-20：MSVC Debug/Release 各 15/15 CTest 通过。新增独立用例覆盖第三方有状态节点注册、按连线调度、静态 Gain、文件分块复用、重建状态、小块能力预算、工厂误配、节点错误/NaN 定位、故障全回调静音与锁存、准备失败资源释放和 CLI 能力发现。回调零 new 检查仅针对被测 C++ 路径，不涵盖全部系统分配。

子代理交叉审查发现并修复了小块能力被 compile 默认值误拒绝、stop 保留处理器资源、非法状态码和停止原因分类问题；实现者之外的代理复核了修复及测试。

本轮通过 --graph 实机静音探测：CABLE Output → Realtek，旁路图 3 秒（采集 144480、输出 147360 帧），Gain 图 5 秒（采集 241440、输出 243360 帧），两次丢帧/欠载均 0、定时正常停止。输入峰值均 0；只有设备回调和新 Graph 路径得到验证，有声效果与上面的未覆盖项目仍不标记完成。

未实现：实时分支/混音、异步和变长输出、ASR/GPU/云端节点、实时录音、Tauri 实时控制、热改图、Workflow/AI 工具接入。后续优先补 F02 受控任务接口，同时补人工设备验收；不为增加音效不断扩大硬件实现范围。
