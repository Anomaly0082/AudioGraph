# Graph 核心接口 v1

本文说明已实现的同步离线入口；实验契约另标注，不将计划功能描述成已经完成。

现在还支持 [同步离线分块 Graph](streaming-graph.md)。下文的 DataValue/execute 描述主要针对整段同步 DAG；分块采用 AudioStream 端口和独立流式接口。

## 配置和执行

配置由 `schema_version: 1`、非空 nodes、connections 和可选 exports 组成。节点由唯一 id、已注册 type 和 parameters 定义。连接使用 from/to，各含 node 和 port。导出使用 name/node/port。

整段处理过程：GraphCodec → GraphDefinition → validate_graph → SyncGraphExecutor::compile → execute。
分块处理过程：GraphCodec → GraphDefinition → validate_stream_graph → StreamingGraphExecutor::compile → execute(block_size)。

`--validate` 校验配置结构、参数、节点/端口引用、类型、必需输入、导出和环路；不创建节点、不验证 WAV 是否存在、不写产物。运行前 CLI 另外检查内置文件节点目标，最终创建文件采用独占创建。

节点数组顺序不表达数据依赖。同一配置的拓扑调度顺序稳定，但独立节点不保证按 nodes 数组全局排序；改变节点或连接声明顺序可能改变无依赖节点的文件副作用顺序。所有节点都会执行，exports 不裁剪图。

JSON Schema 为编辑器提供结构提示；最终以 C++ 检查为准。运行时还拒绝重复 JSON 键、未知字段、超过 4 MiB 或 64 层的文档、非整数编码的版本、字符串 NUL 和非法参数。错误字段路径采用 JSON Pointer（例如 `/nodes/0/parameters/gain_db`）。

路径采用 UTF-8 JSON；解析为绝对路径后保存。相对路径以配置文件目录为基准，内存解析要求显式绝对 base_directory。Windows 的 `D:foo`、`\foo` 等半限定路径拒绝。保存后移动配置不会自动改变其中已绝对化的路径。

## 节点与执行器

NodeRegistry 保存权威端口描述与参数 Schema，工厂创建 ISyncNode。相同契约的新增节点只需注册，不要求 Executor 按具体类分支。

流式节点通过 register_stream_type 注册，create_stream 创建 IStreamNode，并验证 Source/Processor/Sink 角色及实际接口。流式必须通过流工厂，不能伪装成普通同步 Node。能力发现附加 stream_role，旧字段保留。

```cpp
OutputValues execute(const InputValues& inputs, ExecutionContext& context);
```

容器映射端口 ID 到 DataValue：Audio/Number/Text/FilePath。Audio 对应 `shared_ptr<const AudioClip>`，包括 float32 交错样本、采样率和声道数。发布数据后生产者也不得通过自己保留的可写别名继续修改。音频/文本/数值可以使用相同函数签名，但每个端口仍按声明验证。

节点内部实现不依赖相邻节点或具体 Executor。需要外部资源时由 Node 调用服务；当前九个内置节点不使用 GPU 或网络服务。

每次执行创建新实例，防止上次任务状态泄漏。输入默认只读；Gain 创建新音频，Peak 共享读取。Executor 检查输出必需性、类型、空音频指针、音频格式和 NaN/Inf；没有声称支持任意动态数据类型。新增 DataValue 类型目前需要修改类型定义并编译。

可选输出允许省略；若下游必需输入因此缺失，在调用该下游前报错。选择导出但未产生的可选输出也无法返回成功结果。

ExecutionContext 借用取消标志，调用者必须保证标志在同步 execute 返回前有效；节点前后检查取消，内置长循环定期检查。阻塞文件系统调用无法保证立即被中断。

## 文件和结果

输出文件默认不覆盖；输入输出同路径、重复目标和缺失父目录在内置预检中拒绝。新增文件节点需自行采用适当的文件目标策略，不意味着任意插件的所有副作用都能自动分析。

PCM16 写出进行舍入和饱和；内部浮点可以超出归一化范围。Peak 是编码前采样峰值，不是真峰值测量。WAV Output 的 clipped_samples 表示超出 PCM16 归一化范围 [-1, 32767/32768] 的样本数，可通过 exports 获取。

执行成功：

```json
{"schema_version":1,"success":true,"outputs":{"peak":{"type":"Number","value":0.5}}}
```

Text/Number/FilePath 返回 type/value；Audio 只返回采样率、声道和帧数摘要，需连接 wav_output 才保存实际音频。暂未提供独立 Artifact Store。

失败输出 success=false、errors 数组，包含 code/message 和适用的 node_id、port_id、parameter_id、field_path。退出码非零，stderr 保留人类可读错误，stdout 只有一个 JSON 文档（--help 除外）。未知异常封装为 execution_failed。失败或取消可能留下新文件；无文件事务回滚，已存在文件受到保护。

## 异步和旧流式契约实验

`experimental/execution_contracts.h` 分开描述输入粒度、调用方式和实时期限。实验的 IAsyncNode 使用拥有型 InputValues、共享 AsyncContext/stop_token 和 future；IStreamingAudioNode 提供 prepare/push/finish/reset，借用输入、拥有输出。

promise 门控测试验证调用返回、数据持有、协作取消和候选 task-id 迟到结果隔离；分块测试验证尾部排空和状态重置。这些 experimental 接口允许内存分配，future 等待/析构可能阻塞，不能在实时音频回调中使用；它们尚未接入 NodeRegistry 或正式 Executor，也没有真正的异步任务调度服务。正式离线流采用另一个同步 emit 契约，见 streaming-graph.md。
