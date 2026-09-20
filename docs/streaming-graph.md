# P2：同步离线分块 Graph

## 范围与架构判断

先以文件输入输出验证分块执行是合理的：它可以复用当前可配置 Graph，提前检验借用数据、跨块状态、结束排尾和取消语义，而无需首先接设备或云端。

本阶段复用 GraphDefinition、参数 Schema、NodeRegistry、JSON 和结果协议。新增独立 StreamingGraphExecutor 及流式节点工厂。原 SyncGraphExecutor 继续处理整段 AudioClip 和文本 DAG。两者共享控制模型，但数据执行契约不同，不自动混接。

这不是实时麦克风产品，也没有异步工作线程。离线文件按块读取，下游同步消费完成后才读取下一块，形成自然的背压。没有累计整段录音的执行器队列；内存取决于块大小、声道和节点自身的有界工作状态。

## 使用

```powershell
cmake --preset windows-msvc
cmake --build --preset debug --parallel
ctest --preset debug

.\build\Debug\graph-demo.exe --describe-node stream_gain
.\build\Debug\graph-demo.exe --graph .\examples\graphs\stream-gain.json --validate
.\build\Debug\graph-demo.exe --graph .\examples\graphs\stream-gain.json --block-size 256
```

准备 PCM16 WAV 到配置文件目录的 input.wav，或修改 path。输出 stream-gain.wav 默认不覆盖。运行选项 block-size 默认 256 帧，允许 1～65536；不适用于整段图。它统一约束整个流图每次交付的最大块长，不是每个节点各自的参数。

JSON 仍为版本 1，同样描述节点、连接和 exports。节点类型和端口声明确定使用哪种执行器；无需人为排列 nodes 数组来指定调用顺序。

```text
wav_stream_input → stream_gain → wav_stream_output
       AudioStream      AudioStream       ↓
                             path / frames_written / clipped_samples
```

流类型 AudioStream 与整段类型 Audio 区分。AudioStream 不放进 DataValue，不作为最终 JSON 数据导出；sink 正常结束返回 FilePath/Number/Text 摘要。首版只接受一条完整线性链、一个源、零到多个处理器、一个 sink，最多 128 个节点；分支、多输入汇合、循环及混合执行契约明确拒绝。

## 节点接口

实际声明在 `include/audioprocess/streaming_node.h`。

| 角色 | 生命周期 | 数据处理 |
| --- | --- | --- |
| Source | open(max_frames, context) 返回格式 | read(context) 返回借用块；nullopt 表示 EOS |
| Processor | prepare(input_format, max_frames, context) 返回固定输出格式 | push(block, emit, context)，finish(emit, context) |
| Sink | prepare(format, max_frames, context) | push(block, context)，finish(context) 返回摘要 |

处理器允许输出不同的固定格式；Executor 将输出格式传给下一级并逐边验证。接口预留这一能力，当前内置 Gain 保持格式，没有实现重采样算法。

AudioStreamBlock 是只读交错 float32 视图，含 frame_count、channel_count、frame_position。source 的视图有效至下一次 read；处理器输入和 emit 输出在同步调用期间有效。需保留历史的节点必须复制数据到自己持有的状态。

emit 必须同步调用，禁止保存到后台线程或在 push/finish 返回后调用。一次 push 可以不调用 emit（暂时没有结果），或多次调用 emit（每块不超过 max_frames）；零帧块是错误，不能代表等待或结束。

每个节点的输出时间线从帧 0 开始连续累计。缓冲节点排出的块位置可以不同于当前输入块位置；Executor 分别校验各输出流的帧位置、大小、声道、有限采样和溢出。

## Executor 调度

1. 纯校验图结构、参数和导出，不调用工厂、不读写文件。
2. 执行时新建全部节点并验证工厂返回的角色/接口。
3. open 源，依次 prepare 处理器，最后 prepare sink。
4. 读取一个块，同步调用下游 push；emit 继续传递到后续节点。
5. 源返回 EOS 后，按上游到下游顺序调用每个处理器的 finish，其尾部继续经过下游处理器。
6. 全部尾部传播结束后，调用 sink.finish，检查并发布最终摘要。

回调嵌套深度由线性链长度决定，因此明确限制最多 128 节点。配置结构无效时不会产生输出文件；运行时文件、模型或处理错误仍可能发生。

## 停止与资源

每次 execute 创建新节点，第二次执行不继承前一次内部状态。Executor 和内置长循环检查取消标志；取消为协作式，无法保证立即中断正在阻塞的文件系统调用。

取消/异常不再调用算法 finish 排尾，也不返回成功摘要。RAII 释放文件和缓冲资源。WAV 写入器析构可能回填已经写入片段的文件头，这是容器收尾，不是继续运行算法排尾；部分新文件保留，不删除用户文件，不宣称文件事务回滚。

## 与之前实验接口的关系

experimental 目录中的 future 和 vector<AudioClipPtr> 方案仍仅用于候选契约测试。正式离线流采用同步只读块加 emit，避免在节点之间累积整段输出。真实异步流、设备时钟、线程队列、丢帧策略和硬实时内存约束仍需后续设计。

本次保持 Node 处理、Executor 调度、Node 调用外部 Service 的分工；增加的是流式执行契约，没有引入上层 Workflow 或训练功能。
