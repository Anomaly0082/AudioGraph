# Windows 实时设备桥接原型

本阶段验证真实设备边界：WASAPI 输入 → 有界队列 → Gain → WASAPI 输出。它是专用 `RealtimeSession`，不是通用实时 GraphExecutor；不改变已有整段和离线分块 Node 接口。后续需单独设计实时节点准备/执行契约、编译计划和预算检查，才能接入 GraphDefinition。

## 使用

在项目目录构建后运行：

```powershell
.\build\Debug\realtime-cli.exe --list-devices
```

从 `inputs` 和 `outputs` 中分别复制完整 `id` 到 `examples/realtime/session.json`。必须选择明确端点；设备名称、列表下标不能代替 ID。不会修改系统默认音频设备，也不自动回退到其他端点。

```powershell
.\build\Debug\realtime-cli.exe --config .\examples\realtime\session.json --validate
.\build\Debug\realtime-cli.exe --config .\examples\realtime\session.json --probe --seconds 10
# 仅在戴好耳机、调低输出音量后试听；不要与 --probe 同时传入：
.\build\Debug\realtime-cli.exe --config .\examples\realtime\session.json --monitor --seconds 10
```

默认是 probe：采集和处理照常进行，但送到设备的音频强制为零。不会保存音频文件。只有命令行 `--monitor` 能开启实际输出，配置文件不能开启。时长默认 10 秒，范围 1～3600 秒；Ctrl+C 请求停止，设备 stop/uninit 由控制线程完成。

也支持 `--input <id> --output <id> --gain-db -6 --target-frames 960 --period-frames 256`，不能与 `--config` 混用。容量仅在 JSON 中设置。`--validate` 不访问设备，成功只表示配置合法，不代表 ID 当前可用。

VB-CABLE 中，通常应把本程序输出指向播放端 `CABLE Input`，其他应用从录音端 `CABLE Output` 读取。名称易混淆，以设备枚举方向为准；若输出列表没有对应端点，本程序不会自行创建或启用设备。

## 配置约束

| JSON 字段 | 规则 |
| --- | --- |
| schema_version | 必填整数 1 |
| input_device / output_device | 必填非空 UTF-8 ID，不允许 NUL |
| gain_db | 默认 0，有限数，范围 -24～12 dB |
| capacity_frames | 默认 4096，4～1048576 的 2 次幂 |
| target_frames | 默认 960，范围 2～capacity_frames-2 |
| period_frames | 默认 256，范围 32～2048；是请求值，不保证设备采用 |

拒绝未知字段、重复键、超过 64 KiB 或 8 层的配置。内部固定 48 kHz float32 单声道处理，输出复制为双声道；miniaudio 负责原生格式/采样率/声道适配。当前不是保留立体声的通用处理链。

## C++ 边界与线程

- `realtime_config.*`：JSON 解析与纯配置校验，无设备访问。
- `realtime_session.*`：控制线程枚举、初始化、启动和停止 WASAPI 设备；miniaudio 隐藏在 PImpl 后面。
- `realtime_bridge.*`：设备无关的桥接核心；采集线程写入，播放线程读取并执行 Gain。构造时预分配环形队列，回调内不主动分配、不访问文件、不记录日志、不等待其他节点。
- `apps/realtime_cli/main.cpp`：参数、安全模式、停止轮询和机器可读结果。不属于 Tauri 前端；本阶段桌面仍使用原离线 sidecar。

Bridge 是单生产者/单消费者（SPSC），capture/render 各仅允许一个调用线程。控制线程可以读取近似统计和修改 Bridge 的 gain/mute；Session 当前没有运行中参数更新入口。reset/析构必须等待回调全部停止。Session 公共方法由同一控制线程串行调用。

启动先输出静音，累积目标水位后播放。队列满时丢弃新输入中放不下的后缀；欠载时输出静音并重新预缓冲，不无限积压。独立设备时钟通过水位反馈与线性插值做 ±0.5% 的轻微速率补偿，不是高品质音乐重采样算法。增益更新约 10 ms 平滑；输出限制在 [-1,1]，非有限值被净化并计数。

显式选定的设备停止、路由改变或中断时，回调只设置故障标记，控制线程随后停止两端；不会自动切换默认设备。正常结束和初始化中途失败均清理已创建的设备。不保证阻塞的系统设备 API 能在固定时间内返回。

## 返回和指标

stdout 输出一个最终 JSON，stderr 每秒输出 metrics JSON；启动失败另有可读错误。帮助输出除外。`success` 仅表示没有已检测故障且两个方向都收到回调，不保证有语音、不保证无削波或满足延迟预算。失败退出码非零。

关键指标：`capture_frames`、`render_frames`、`dropped_frames`、`underflow_frames`、`buffering_silence_frames`、`clipped_samples`、`capture_peak`、`output_peak` 和 `resample_ratio`。`software_queue_latency_ms` 只计算软件队列，不含设备缓冲/系统/算法整体延迟，不能当作嘴到耳的延迟。各字段是近似快照，不保证同一时刻。

受限环境可能允许设备枚举但阻止初始化，返回通用 `Invalid argument (-2)`。若发生，可在正常用户终端用相同 `--probe` 命令对照；不能把所有 -2 都解释为权限问题，也不应因此默认要求管理员运行。

## 验证与未覆盖项

2026-09-20：MSVC Debug/Release 各 13/13 CTest 通过。新增测试覆盖有界缓冲、欠载、静音、Gain、非有限输入、并发生产消费、模拟时钟漂移及配置/CLI 拒绝行为。回调测试检查 C++ new/new[] 分配，不等于拦截了系统或第三方库的全部内存分配。

沙箱外静音实测：WO Mic → Realtek 3 秒，采集 145440 帧、输出 147840 帧；CABLE Output → Realtek 20 秒，采集 960480 帧、输出 962400 帧。两次均无队列丢帧/欠载，均有 2400 帧预缓冲静音。设备原生均 48 kHz，周期 480 帧；CABLE 原生双声道，经适配进入单声道核心。输入峰值为零，因此只证明回调及静音链路，不证明语音保真。

尚未验收：真人有声试听、真实异采样率设备、拔插故障、Ctrl+C 实机行为、长时间稳定性、端到端延迟、虚拟播放端输出。本机未枚举到 CABLE Input 播放端。自动测试不会打开麦克风，也不会播放音频。

未实现：任意实时 Graph、实时分支/混音、ASR/GPU/云端节点、实时录音、Tauri 实时控制、Workflow/AI 工具接入。下一阶段应先补人工设备验收，再把经验证的实时处理契约接入 Graph，而非直接在回调里调用离线 Executor。
