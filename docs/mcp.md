# 最小本地 MCP 适配层

P7 在 `apps/mcp` 提供本地 stdio MCP 服务，使用官方 `@modelcontextprotocol/sdk` 1.30.0 和 Zod 4.6.5，依赖版本由 package-lock.json 固定。Node 仅做协议适配和子进程通信，音频仍由 C++ control-cli / TaskService / GraphExecutor 执行。

本阶段只开放 offline（整段）和 streaming（离线分块），不开放实时设备、麦克风、有声输出、任意命令、Python、插件加载、Workflow 或训练。MCP 服务自身不调用 LLM，也不需要模型 API key；它等待 AI 客户端发现和调用工具。

## 准备与运行

需要本机 Node.js 22+、已构建的 C++ control-cli，以及一个现存的音频工作目录。建议使用专门的数据目录，例如 C:\AudioGraphWork，不要把用户主目录、整个磁盘或源码仓库根目录作为音频工作目录。

在 VS2022 Developer PowerShell 的仓库根目录构建引擎：

```powershell
cmake --preset windows-msvc
cmake --build --preset debug --target control-cli --parallel
cmake --build --preset release --target control-cli --parallel
Set-Location .\apps\mcp
npm ci --ignore-scripts
npm run check
npm test
```

适配层不依赖 Tauri，不需要启动桌面窗口。直接启动时它等待 MCP JSON-RPC，不是交互式人类命令行：

```powershell
node .\src\main.mjs --workspace C:\AudioGraphWork --engine C:\AAAProject\AudioProcess\build\Release\control-cli.exe
```

两个路径由可信宿主启动时指定，工具参数不能修改；workspace 必须已经存在，engine 是 control-cli 的绝对路径。不要把音频任务写成任意脚本传给服务。

## 客户端接入

`apps/mcp/client-config.example.json` 是支持 mcpServers JSON 格式的客户端配置示例，不是通用客户端自动安装器。将它按你实际客户端的配置格式添加，并替换 Node、server、workspace 和 engine 路径；如客户端的 PATH 找不到 Node，请使用 node.exe 的绝对路径。

该示例不会被本仓库自动激活，也没有改写全局客户端设置。客户端连接后应完成 MCP 握手，并能列出下方工具；当前聊天是否能直接调用，还取决于客户端是否已挂载/重连，不能仅凭测试通过就声称真实 AI 已接入。

## 工具及任务语义

| 工具 | 用途 |
| --- | --- |
| audio_capabilities | 当前适配层模式、权限和限额 |
| audio_list_nodes | 可用节点的精简目录 |
| audio_describe_node | 指定节点的端口、参数及约束 |
| audio_validate_graph | 校验完整 Graph 和执行选项，不运行节点 |
| audio_start_task | 提交 Graph，返回 task_id 和当前状态 |
| audio_task_status | 查询当前任务状态 |
| audio_cancel_task | 请求协作取消，不回滚已产生文件 |
| audio_task_result | 获取终态结果或失败详情 |
| audio_release_task | 释放终态内存记录，不删除输出文件 |

具体输入以客户端 tools/list 返回的 JSON Schema 为准。图仍采用 Graph v1，参数及模式的业务校验由 C++ 最终裁定；不兼容模式、未知字段、越界路径或已存在输出会被拒绝。

校验不会检查输入音频是否存在或输出文件是否已存在；这些运行条件在执行阶段检查。已有输出会使任务失败，而不是让纯 Graph 校验产生文件副作用。

Graph、节点、连接及 exports 的结构会在工具 Schema 中明确列出；节点 parameters 的字段和值仍交给 C++ Registry 校验。原始输入拒绝重复键、无效 UTF-8，以及 graph.schema_version 的浮点/指数写法，避免协议转换静默改变配置含义。

MCP tools/call 返回只表示这次工具调用结束，不代表音频任务已经结束。start 后根据 task_id 查询；一个活动任务、至多16条保留记录等限制沿用 [P5 控制协议](control-api.md)。MCP 请求级取消不自动等同于已提交业务任务的取消，使用 audio_cancel_task 管理任务。

适配器返回的后端结果同时提供文本和 structuredContent；SDK在进入回调前拒绝工具输入Schema时，可能只返回isError及错误文本。操作错误会标记 isError；查询状态时发现任务 failed，不等于“状态查询本身失败”。任务结果中的 failed/cancelled 不能冒充音频处理成功；后端错误保留节点、端口或字段位置，供模型修正后重新提交。

## 第一个实际场景

先将一份 PCM16 WAV 放进工作目录，例如 input.wav，再让已连接的 AI 客户端完成：

> 查询 Gain 节点的参数，用离线 Graph 将 input.wav 的振幅降低约一半，保存为一个不存在的新文件 quieter.wav。先校验图，再执行并检查任务结果，告诉我输出路径。不要覆盖文件，不使用脚本，不调用音频设备。

可用节点为 wav_input → gain → wav_output；约 -6.0206 dB 表示振幅减半，不等于听感响度减半。Graph 应通过 exports 导出输出路径；MCP 不回传整段二进制音频，结果文件留在本地工作目录。

模型可以在明确错误后调整 Graph 再提交，但不要因启动响应丢失就自动重发。文件输出不覆盖；失败或取消可能留下部分新文件。

## 生命周期与安全边界

每个 MCP 服务持有一个自己的 control-cli；会话间任务ID不共享，不控制已经打开的 Tauri 会话。服务退出/EOF时关闭子进程stdin，优先让C++协作取消；超时仅终止自己创建的进程并输出诊断，不按名称杀进程、不重启重试任务。

P5 请求最多4MiB/64层、8个待处理请求，后端响应最多8MiB；MCP外层帧最多4MiB/128层，进入C++前仍受更严格的P5深度约束。RPC等待默认15秒；关闭stdin后默认给C++1秒协作清理，再终止自有进程，若仍无法确认退出则明确报错。预发送校验拒绝不会损坏有效连接。

宿主的路径范围继承 C++ 控制协议。所有工具调用仍应受 AI 客户端的用户授权策略约束；工具 annotations 仅是提示，不是强制权限机制。已存在文件不覆盖，但这不是 OS 沙箱：不能隔离恶意 C++ 节点，也不能消除外部进程替换文件/链接的检查使用竞态。因此应选择专用数据工作目录。

目录中也可能使用 text_output 创建新文本文件，不是只允许 .wav 扩展名的文件类型沙箱；不要把含源码仓库钩子、客户端配置或个人敏感文件的目录整体授权给音频任务。

stdout 仅用于 MCP 协议，诊断走 stderr。适配器对大小、深度、等待数量和超时做限制；传输失效后拒绝等待者且不自动重连，任务状态可能不确定。外部宿主直接强杀、系统崩溃等仍可能打断清理，不保证所有情况下无残留或输出完整。

## 设计依据

遵循官方 [stdio 传输规范](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)和[工具规范](https://modelcontextprotocol.io/specification/2025-11-25/server/tools)，协议生命周期由 [官方 SDK](https://ts.sdk.modelcontextprotocol.io/server) 负责。MCP 是控制接口，不替代 Node/Executor，也不自动提供条件循环或优化算法。

## 验证记录（2026-09-22）

- `npm run check` 通过，MCP 适配层17项测试全部通过；原C++ Debug/Release各19/19回归通过。
- 官方SDK Client通过真实stdio服务完成initialize、tools/list、tools/call；工具schema能发现Graph结构，节点参数由describe取得。
- 真实C++后端处理17帧整段和301帧分块PCM16 WAV，中文/空格文件名通过，输入0.25经约-6.0206dB后输出约0.125；重新解析文件验证格式、帧数和采样，而不只检查success。
- 覆盖越界、不覆盖、错误定位、未知操作/字段、设备/实时范围拒绝、任务结果/释放；模拟后端验证乱序、超限、EOF、超时不重试和只强制关闭自有进程。
- 测试回显并深比较自定义参数，确认没有被Schema静默删除；真实MCP服务正常断开时断言退出码0/无终止信号，不依赖SDK后备强杀。收尾先关闭MCP与后端再删除独占临时目录，避免Windows当前目录锁；只读进程检查无本轮MCP服务或后端残留。没有访问用户音频、设备或全局客户端配置。
- 尚未在真实LLM客户端挂载服务，也没有评估模型是否能稳定自行选择参数或处理复杂任务。上述是MCP客户端互操作与执行链路证据，不是完整AI功能验收。
