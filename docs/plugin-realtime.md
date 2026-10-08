# 实时音频处理器插件（实验能力）

除下文的最小 Gain 示例外，已有[独立 RNNoise 实时降噪插件](plugin-rnnoise-realtime.md)，用于验证有状态分帧算法，支持按现有插件包机制安装，不改变实时 ABI。

新版 SDK 增加 `ag.realtime_audio/1`，与 `ag.whole_sync/1` 分开协商。基础 C ABI 仍为0.1，已有函数表布局不变；旧插件可继续使用整段同步接口。基础 ABI 相同不表示旧宿主支持实时，新插件需新版宿主及带此能力的SDK头。

## 首版边界

- 仅处理器：48 kHz、单声道、交错 float32，一个必需的 `audio` AudioStream 输入与输出，原地处理且帧数不变。
- 变长块为1..maximum_frames，最大声明上限65536帧；实际容量由prepare请求确定，不得假设设备每次提供固定256帧。
- `execution_domain: "realtime"`、`realtime_role: "processor"`、`stream_role: "none"`。必须声明完整realtime_capabilities：format、maximum_block_frames、supports_variable_blocks（true）、offline_drivable。
- 每个节点声明其对应的一种能力。包的capabilities必须等于节点能力集合，允许同包包含分别为whole/realtime的节点，不允许未知、重复能力或版本降级。
- 设备源/终点仍用内置 `realtime_input` / `realtime_output`。实时图是线性链，没有离线Graph的exports；不能直接接入ASR、云端请求、额外Text/Number端口、分支或异步任务。

`offline_drivable`表示可以不打开设备而逐块供数测试，不等于可把该类型混入现有offline Graph。

## 生命周期

通过 `get_node_api(type_id, AG_REALTIME_AUDIO_ID, AG_REALTIME_AUDIO_VERSION, ...)` 取得 `ag_realtime_audio_api`，不要请求whole_sync表来执行实时节点。

1. **create**：控制线程复制参数和环境路径，轻量创建实例。失败输出空句柄；成功后的实例最终必须destroy一次。
2. **prepare**：设备启动前的控制/任务线程，读取已验证资源、分配所有状态与缓冲，接收24字节的 `ag_realtime_prepare_info`（包含格式与最大帧数）。失败也由宿主销毁已创建实例。新会话使用新实例，参数改变需停止并重新准备。
3. **process**：同一实例串行、可重复调用，接收32字节的 `ag_realtime_block`；samples为借用的可写float缓冲，frame_count和sample_count不得修改，不保留指针。
4. **destroy**：仅在设备停止并反初始化、回调完全退出后由控制线程调用，插件释放自己的内存。不是在回调中执行的stop方法。

process必须有界，不能分配/释放内存、等待锁、读文件、联网、访问设备、写日志、解析JSON或计算包指纹。预先准备状态可以跨块保留；所有跨DLL函数必须自行捕获异常。`resource_root`和`data_root`的约定与整段插件相同，但资源只能在准备阶段读取。

## 返回状态与失败

create/prepare仍返回ag_status，可在其调用线程使用错误sink。process返回独立的ag_realtime_status数值域：AG_RT_OK、AG_RT_NOT_PREPARED、AG_RT_INVALID_BLOCK、AG_RT_NON_FINITE_INPUT、AG_RT_NODE_FAILED、AG_RT_NON_FINITE_OUTPUT。不要把AG_CANCELLED等whole_sync状态当成实时状态。

process不提供文本错误回调。未知状态视为节点失败；宿主检查输入/输出有限值，复用桥接层将当前设备输出整块静音并锁存错误，控制线程随后停止会话，不自动直通或重试。可信原生插件不是沙箱；卡死或非法内存访问无法靠此接口安全硬抢占，不能承诺任意第三方代码满足实时期限。

## 示例、构建与验证

开发套件中的 `examples/realtime/` 为独立Realtime Gain，插件ID `org.audiograph.realtime_example`，类型 `org.audiograph.realtime_example.gain_v1`，参数gain_db默认0，范围−24..12 dB。它只链接公开SDK，不链接主程序核心；包格式v2包含入口DLL和nodes.json。

在开发套件根目录执行（WORK_ROOT必须是SDK外的新绝对目录）：

```powershell
cmake -DEXAMPLE=realtime -DWORK_ROOT=C:/Temp/AudioGraph-rt-check -P ./scripts/verify-example.cmake
```

这只编译并用合成音频测试，不开设备。测试host只针对该示例，不能替代自定义插件的验证。分配检测只覆盖宿主适配器和示例所检查的C++ new/delete路径，不证明任意第三方DLL或malloc/系统调用无分配。

安装生成的 `plugin-build/package/Debug/` 完整目录，重启AudioGraph后，在Graph编辑器切换到实时模式。可载入开发套件中的 `examples/graphs/plugin-realtime-gain.json`，占位设备ID必须通过设备面板重新选择并应用。

手动验收先运行静音probe，再以低增益和耳机试听，避免扬声器回授；最后可输出到虚拟音频设备验证跨软件链路。停止、重新启动及改参数后重启处理均需检查。AI工具仍只允许offline/streaming，新增插件不会自动开放麦克风权限。自动化合成音频测试不代表真实设备链路已通过。
