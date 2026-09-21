# 受控任务接口 v1

P5 将已有整段、离线分块和实时执行路径封装成任务服务。C++ `TaskService` 管任务生命周期，`task_runner` 调用相应 Executor/Session，`ControlProtocol` 做 JSON 和宿主权限适配，`control-cli` 维持标准输入/输出连接。没有启动 HTTP 服务，也没有安装 AI/MCP 连接器；Tauri 尚未接入。

## 启动与快速实验

```powershell
.\build\Debug\control-cli.exe --workspace C:\AAAProject\AudioProcess
```

在同一进程中逐行发送 UTF-8 JSON，每行收到一个响应。不要为每条操作启动一个新进程；任务 ID 只在当前进程中有效。以下请求直接提交文本图，不产生文件或访问设备：

```json
{"schema_version":1,"id":"q1","op":"capabilities"}
{"schema_version":1,"id":"q2","op":"tasks.start","mode":"offline","graph":{"schema_version":1,"nodes":[{"id":"source","type":"text_input","parameters":{"text":"你好"}}],"connections":[],"exports":[{"name":"message","node":"source","port":"text"}]}}
```

第二个响应返回 `data.task_id`。下面假设它是 task-1（实际应使用响应值）：

```json
{"schema_version":1,"id":"q3","op":"tasks.status","task_id":"task-1"}
{"schema_version":1,"id":"q4","op":"tasks.result","task_id":"task-1"}
{"schema_version":1,"id":"q5","op":"tasks.release","task_id":"task-1"}
```

结果尚未完成时返回 task_not_finished，应稍后查询状态，不要直接重复提交启动。关闭 stdin/EOF 会请求取消活动任务并等待清理，所以一次性管道发送 start 后立刻关闭输入不能当作后台启动。正常结束应先取消/等待终态，再关闭连接。

## 请求和响应

所有请求包含整数 schema_version=1、非空 UTF-8 字符串 id（最多128字节）和 op。每行最多4 MiB、嵌套最多64层。未知操作/字段、重复键、无效参数被拒绝；坏请求不会使服务直接退出。id 仅关联响应，不提供幂等去重。

操作成功响应：

```json
{"schema_version":1,"id":"q3","success":true,"data":{"task_id":"task-1","state":"running"}}
```

操作失败响应含 success=false 和 errors 数组，沿用 code/message 和适用的 node_id、port_id、parameter_id、field_path。请求不能解析或 id 不合法时 id=null。Graph 字段位置沿用 Graph 文档内部的 JSON Pointer，例如 /nodes/0/parameters/gain_db；不是外层请求的 /graph/nodes/...。

**外层 success 只表示这次控制操作成功。** 查询一个失败任务时，外层仍可为 true，data.state=failed，并带 errors；不要据此外层字段误判音频任务成功。

| op | 附加字段 | 行为 |
| --- | --- | --- |
| capabilities | 无 | 返回操作列表、已注册节点描述、模式、宿主权限和限额 |
| nodes.list | 无 | 返回已注册节点及端口/参数/能力描述 |
| nodes.describe | type | 返回指定节点描述 |
| devices.list | 无 | 获得宿主设备权限后枚举设备，不启动音频 |
| graph.validate | mode、graph、可选 options | 复用图校验，并检查宿主文件边界，不创建节点/输出或打开设备 |
| tasks.start | 同 graph.validate | 验证通过后提交请求快照，立即返回 task_id 和当前状态 |
| tasks.status | task_id | 查询状态和适用错误，不复制音频或大结果 |
| tasks.cancel | task_id | 请求协作取消；已终态时不改变结果 |
| tasks.result | task_id | 仅终态可取，返回 state、result 和适用 errors |
| tasks.release | task_id | 仅删除终态的内存记录，不删除产物文件 |

nodes.list 的 data 沿用旧节点目录结构，内含 nodes；nodes.describe 的 data 内含 node。已注册不代表任意运行模式都支持：根据节点执行契约选择，混合不兼容模式会被拒绝。

## 执行选项

mode 必须显式指定。graph 是现有 Graph JSON 对象，不是脚本或图文件路径；相对 FilePath 基于宿主 workspace 解析。请求可以自行构造、修改和重复提交图，无需节点专属命令。

- offline：整段同步 DAG，options 只能省略或为空对象。
- streaming：离线分块线性图，options 可含 block_frames，默认256、范围1～65536。
- realtime：实时线性图，options 可含 block_frames（默认256）、probe（默认true）、duration_seconds（默认10、范围1～3600）。格式仍限48 kHz单声道；具体块长还受节点能力约束。

block_frames 必须用 JSON 整数表示，256.0 会被拒绝。其他模式的专属字段不能混用。probe=true 仍采集、处理，但最终输出静音；未实现任务运行中调参、热改图或进度百分比。实时统计在结束结果中提供，当前 tasks.status 只返回任务状态。

offline/streaming 的 data.result 沿用 Graph 执行结果（exports 摘要）；Audio 不返回采样数组，需连接文件输出节点保存音频。realtime 的 data.result 包括 stats、device_format、probe、stopped_by；失败可保留诊断统计，取消清除结果负载并返回 cancelled 错误。

## 状态、取消和容量

正常状态：queued → running → succeeded/failed。取消状态：queued/running → cancelling → cancelled。一个活动任务包含 queued/running/cancelling；忙时新任务被拒绝（task_busy），没有无界等待队列。

取消请求与终态发布使用同一状态锁：取消先被接受则取消优先，即使计算稍后返回成功或失败；终态先发布则迟到取消不改变它。queued 任务可能直接取消而不执行 Runner。取消不回滚已生成的文件，也不保证立即中断阻塞的系统调用/不合作的 C++ 节点。

至多保留16条任务记录（包含活动任务）；达到上限报 task_capacity，调用 tasks.release 释放终态记录。任务 ID 在同一服务中递增不复用；重启后无历史恢复。结果 JSON 最大4 MiB，超过报 result_too_large，不长期保留该大结果；这不是执行过程中内存用量的硬限制。

TaskService 的 status/cancel 不等待 Runner 完成。但协议控制线程仍同步解析/验证请求和枚举设备；不能承诺大请求或缓慢设备枚举期间后续消息有固定响应时限。宿主必须持续读取 stdout，输出管道阻塞也会延迟后续请求。

## 权限与安全边界

默认 workspace 为启动时工作目录，也可用宿主 --workspace 指定现存目录。所有声明为 FilePath 的参数（含输入和输出）必须位于该目录内，已有符号链接/目录联接解析后仍要在范围内；启动提交前和 worker 执行前重复检查。图校验为此读取路径元数据，但不读取音频内容或创建产物。路径比较保守，不保证接受 Windows 下所有大小写/短路径别名。

默认拒绝 devices.list 和 realtime tasks.start。宿主 --allow-devices 才允许设备访问；若需有声输出，还必须同时提供 --allow-monitor，然后请求 probe=false。JSON 请求无法开启宿主权限。graph.validate 可在无设备权限时验证实时图，因为它不会打开设备。

**这不是 OS 沙箱。** 不能防止其他进程在检查后替换文件/链接，也不能隔离恶意 C++ 节点。节点可用能力由可信宿主注册；当前进程适配器只注册内置节点，不接受 DLL 路径、Shell、Python 或 eval。旧 CLI 是面向本机开发者的工具，不自动获得这个入口的路径/设备限制。此服务不应直接暴露到不可信网络。

文件仍不覆盖，失败/取消可能保留部分新文件。未实现图历史回滚、事务、审计数据库、Artifact Store、Workflow 或训练；AI/MCP/Tauri 适配是后续步骤，不把“已有协议”当成“已接好 AI”。

## C++ 阅读顺序

`task_service.h → task_service.cpp → task_runner.cpp → control_protocol.cpp → apps/control_cli/main.cpp`。

TaskService 不依赖 JSON 库，Runner 可注入用于独立测试；Graph/Node/Executor 不知道 UI、MCP 或标准输入的存在。实时 Session 从启动到停止都在同一 worker 上，设备回调仍只运行准备好的实时计划。

“异步提交任务”只表示控制者不用等任务执行完；不表示 Graph 中的节点已变成异步节点，也不意味着云端/GPU 并发调度已经实现。

## 验证记录

2026-09-21：MSVC Debug/Release 各 19/19 CTest 通过。新增独立测试覆盖门控任务状态/取消竞争、终态稳定、请求快照、记录/结果容量、析构取消清理，以及真实文本/WAV 的同步和分块执行。实时自动测试只在节点准备阶段故意失败，不打开设备。

协议/进程测试覆盖多请求连接、错误与超长行后的恢复、任务结果、EOF 正常退出、默认设备禁止，以及 Windows Junction 越出 workspace 时被拒绝。活动任务析构清理另由门控服务测试验证，不把终态 EOF 测试当作所有设备故障覆盖。

另做了新接口的静音实时取消探测：宿主显式允许设备、请求 probe=true，绑定 CABLE Output/Realtek，约1秒后发送取消；观测 running → cancelling → cancelled，关闭 stdin 后进程正常退出。取消语义不保留统计负载，因此这次不声称验证了采样数量、音质或延迟；真人有声、设备拔插和固定取消时限仍待专门验收。
