# 开发独立插件节点

本文用于独立 SDK 开发包，也适用于仓库内开发。开发包根 README 给出工具链和构建命令；包成员、依赖及资源限制见[插件包规范](plugin-packages.md)。以独立示例为起点，通过 `find_package(AudioGraphPluginSDK 0.1.0 EXACT CONFIG REQUIRED)` 和 `AudioGraph::PluginSDK` 使用公开头 `audiograph/plugin.h`，无须链接 AudioGraph 核心。

## 当前支持的节点描述

`nodes.json` 根字段为 `schema_version`（1）、`plugin_id`、`plugin_version`、`experimental`（true）、`production_ready`（false）和 `nodes`。这与 manifest 的包格式版本独立，不能因为 manifest 使用 v2 就把节点描述改成 v2。静态描述与 DLL 的 `describe_nodes` 返回描述必须一致；示例构建会把同一份 JSON 嵌入 DLL。

每个节点使用以下字段，不要添加自创字段：

- `typeId`、`displayName`、`description`。
- `execution_domain: "synchronous"`、`stream_role: "none"`、`realtime_role: "none"`。
- `inputs`、`outputs`：数组，每项只有 `id`、`type`、`required`；类型大小写严格为 `Audio`、`Number` 或 `Text`，`required` 为布尔值。每方向最多 32 个，方向内 ID 唯一。
- `parameters`：最多 32 项，ID 唯一。字段为 `id`、`type`、`required`，以及可选 `description`、`default`、`minimum`、`maximum`、`unit`、`enum`、`integer_only`。类型为小写 `number`、`text` 或 `boolean`；默认值必须匹配类型，范围值为数字，`integer_only` 为布尔值，`enum` 是最多 64 项的字符串数组。
- `plugin`：`id`、`implementation_version`、`capabilities`、`experimental`、`production_ready`。身份和版本必须匹配根描述及 manifest；能力只能是 `[{"id":"ag.whole_sync/1","version":1}]`，标志必须分别为 `experimental: true` 和 `production_ready: false`。

`typeId` 必须以 `plugin_id + "."` 开头。以示例 ID 的字符形式为起点；端口和参数 ID 最多 64 字节，typeId 最多 128 字节，displayName 最多 256 字节，description 最多 2048 字节。宿主会拒绝未知字段及不支持的类型。当前没有 FilePath 端口或参数、实时流或异步能力；头文件里的 `AG_FILE_PATH` 标签不代表当前宿主已开放该功能。

## 默认参数怎样送到插件

实际宿主在调用插件 `create` 前按节点描述标准化参数。Number、Text、Boolean 参数遵循相同规则：

| Graph 提供情况 | 宿主行为 |
| --- | --- |
| 显式提供合法值 | 使用该值，不用默认值替换 |
| 显式提供错误类型、越界值或未知参数 | 校验失败，不回退到默认值 |
| 省略参数且描述声明了 `default` | 将默认值补入参数表，即使 `required` 为 true 也可补齐 |
| 省略参数、无默认值且 `required: false` | 参数表中不出现该 ID，不补零、空字符串或 null |
| 省略参数、无默认值且 `required: true` | 校验失败，不进入插件创建 |

例如节点描述中有 `target_peak_db`（默认 −3）和 `max_gain_db`（默认 12）两个可选参数：Graph 写 `"parameters": {}` 时，`ag_create_info.parameters` 包含这两个默认值；只传 `target_peak_db: -6` 时，收到 −6 和 12。参数通过 ID 匹配，不要依赖数组顺序或假定所有可选参数都一定存在。插件仍须验证收到的数据；直接调用 ABI 的独立测试宿主若没有执行这套标准化，不能据此推断实际宿主会省略已声明默认值的参数。

## Number 输出与 Graph 导出

节点在 `outputs` 中声明 `{"id":"applied_gain_db","type":"Number","required":true}`，执行时以该 ID emit `AG_NUMBER` 的有限 double。Number 和 Audio 一样是普通数据端口，可以连接到兼容的输入，也可以直接成为 Graph 导出，不必先转成文本或文件。

例如已经安装归一化插件、Graph 中节点 ID 为 `normalize`，WAV 输出节点 ID 为 `output`，Graph 顶层可写：

```json
{
  "exports": [
    {"name":"output_file","node":"output","port":"path"},
    {"name":"applied_gain_db","node":"normalize","port":"applied_gain_db"}
  ]
}
```

这是导出字段片段，不是完整 Graph。`node` 是 Graph 内实例 ID，不是节点 `typeId`；`port` 必须是该节点声明的输出端口 ID；`name` 是结果中的键，不能重复。导出同一端口不妨碍它同时连接下游。

成功的 Graph 执行结果中，标量以类型和值表示，例如：

```json
{
  "outputs": {
    "output_file": {"type":"FilePath","value":"<实际输出路径>"},
    "applied_gain_db": {"type":"Number","value":6.0}
  }
}
```

数值 6.0 仅为格式示例，不是固定结果。任务、Workflow 或 AI 工具可能在外层再包装执行结果；这里展示的是 Graph 结果内的 `outputs`，不是完整工具响应。Number 导出是结构化数值，不会自动创建 TXT/CSV 文件；`output_file` 则引用内置 WAV 输出节点已生成的文件。失败运行不能当作取得了成功导出。

## 改成自己的插件

同步检查所有身份来源：

1. `nodes.json` 根 `plugin_id`/`plugin_version`，每个节点的 `typeId` 和 `plugin.id`/`implementation_version`。
2. C++ `ag_plugin_get_api` 返回的插件 ID/版本，`get_node_api` 的节点类型分派，以及算法使用的端口和参数 ID。
3. `cmake/manifest.json.in` 身份、入口文件名，CMake 目标输出名和打包脚本；辅助 DLL 文件名必须以自己的 `plugin_id + "."` 开头。
4. Graph 中的节点类型与参数；资源、依赖及每个成员的 SHA256 必须通过重新构建/打包更新。

保持精确导出符号 `ag_plugin_get_api`、调用约定、结构布局、`AG_ABI_MAJOR`/`AG_ABI_MINOR` 和能力常量。不要用修改 ABI 或宿主的方式实现新算法。独立示例是 Gain 和模拟 ASR；模拟 ASR 不含真实识别、模型推理或网络请求。新算法须有自己的行为测试；开发包里的 SDK host 只测试原样示例。

## 生命周期和计算

`create` 只做轻量参数验证和实例准备，失败时写空实例。模型加载、GPU 初始化、HTTP 请求等耗时工作放到 `run`，受其取消和截止时间预算约束。实例的 `run` 只执行一次，同一实例不会并发执行；不同实例可能并行。成功 create 后宿主调用一次 destroy，由插件释放自己的内存。所有导出函数和回调必须捕获自身 C++ 异常，不能穿过 C ABI。

参数已经标准化，但插件仍应检查类型、数量和算法约束。输入是只读借用视图，仅在 run 期间有效。Audio 是交错 CPU float32，样本数等于帧数乘通道数，数值须有限；Number 也须有限，Text 是带长度的 UTF-8，不能假定以 NUL 结尾。

取消是协作式的：定期检查 `is_cancelled` 和 `remaining_ms`，对外部调用设置有限超时，返回相应失败状态。零剩余时间表示超时，`AG_NO_DEADLINE` 表示没有宿主期限。后台线程可查询这些回调，但必须在 run 返回前停止访问所有宿主指针和回调。不能留下继续使用借用数据的任务。

仅在 run 的调用线程上调用输出 sink，每个输出 ID 最多 emit 一次；非 `AG_OK` 时停止。emit 返回前宿主已经复制数据，所以插件仍拥有原始缓冲区，不能要求宿主释放它。遵守 context 的输入/输出字节及数量限额，零容量不是无限制。输出只有在 run 成功及最终检查通过后发布；文件写入或网络操作不会自动回滚。错误 sink 只在当前 create/run 调用线程上报告，消息应短且不含秘密。不要写进程 stdout，生产 worker 使用它传递控制协议。

## 模型、服务和资源

`environment.resource_root` 是已安装插件包根目录的绝对 UTF-8 路径，按只读约定使用。模型或配置从它下面的 `resources/...` 解析，不从进程当前目录查找。`data_root` 是插件/工作区专用的可写数据位置，可存缓存或运行数据；需要长期使用的路径必须在 create 中复制。

随包模型和私有 DLL 都须在 manifest 中声明。容量见包规范；当前没有大模型外部绑定、自动下载或凭据系统。需要服务调用时，插件自行实现客户端及超时/取消；SDK 没有提供服务、GPU或识别框架。原生插件属于可信代码，这些路径约定不是 OS 沙箱。能力、模型容量或授权不足时应报告缺口，而不是绕过工作区或权限限制。

## 交付验证

先用未修改示例确认工具链和 SDK，再验证新算法的正常结果、空/异常输入、参数边界、取消、超时、输出容量和依赖/资源缺失。重新打包后，在实际 AudioGraph 的插件管理界面安装生成的插件目录并重启，确认节点发现、Graph 执行和错误提示。安装目录应只含 manifest 声明的成员；修改任何成员后重新生成指纹并重启。独立构建成功不等于实际宿主兼容性或生产安全已经验证。
