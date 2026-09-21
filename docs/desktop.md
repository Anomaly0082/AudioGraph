# 最小桌面控制台

P6 用 Tauri/React 操作已有受控任务接口。C++ 仍负责 Graph 校验、Node 和 Executor，Rust 仅管理固定的 control-cli 子进程、请求/响应和有限的配置文件读写。没有拖拽节点编辑器，也没有新增 DSP、Workflow 或 AI/MCP 连接器。

## 构建

在 VS2022 Developer PowerShell 的仓库根目录：

```powershell
cmake --preset windows-msvc
cmake --build --preset debug --target control-cli --parallel
Set-Location .\apps\desktop
npm ci
npm run build
npm test
npm run tauri -- dev
```

已有 node_modules 时无需反复 npm ci。CMake 会把 control-cli 复制到 src-tauri/binaries，Tauri 再将它放到桌面 EXE 旁。修改 C++ 后必须重新构建并重新启动 Tauri 流程，不能继续使用上一次运行中已经启动的旧子进程。

生产前端嵌入式调试包可用 `npm run tauri -- build --debug --no-bundle`，它不依赖 Vite 开发服务器。Rust 检查可在 src-tauri 中执行 `cargo test --offline`（前提是依赖已经缓存）；真实 sidecar 测试的运行方式以测试输出和下方验证记录为准。

## 使用顺序

1. 选择或输入现存工作目录，再显式连接。默认设备权限和有声输出权限都关闭。
2. 从无文件副作用的文本模板开始，查看节点的端口、参数和约束。
3. 在 JSON 编辑器修改 Graph，选择 offline / streaming / realtime 模式及适用选项。
4. 校验，再运行；任务区域显示 ID、状态、结果或结构化错误。运行中可取消。
5. 终态记录释放后可以再次执行。断开或关闭窗口会结束自己创建的后台连接，任务不会转为独立后台服务。

本阶段不自动持久化未保存的草稿和连接设置；关闭窗口前请将需要保留的 Graph 另存为文件。已完成结果对应提交时的 Graph 快照，之后编辑不会重新计算结果。

模板只是起点，连接关系由 JSON 决定，不根据节点数组的显示顺序执行。模式/图/选项改变后，之前的校验标记不再有效，真正启动时 C++ 仍重新校验。坏 JSON 或载入失败不会用空内容替换编辑器。

选择实时设备需要先在连接配置中明确允许设备，再连接并刷新设备列表。选择输入/输出后需要显式应用到图里的实时端点，不会自动生成或替换自定义节点。默认 probe 静音；有声输出必须同时有宿主权限和本次运行的非静音选择，试听前应戴耳机、调低音量以防啸叫。

## 配置文件和路径

载入/保存只接受工作目录内的 JSON，有限大小并按 UTF-8 处理；保存为新文件，不覆盖已有文件。文件对话框只提供选择路径，不能扩大 C++ 的音频文件权限。

**所有 Graph 相对文件路径都以连接时的工作目录为基准，不以 Graph JSON 所在文件夹为基准。** 这与旧 graph-demo 的“配置文件目录为基准”不同，遵循 P5 受控接口规则。要使用一个目录下的相对路径配置，应将该目录设为 workspace。

改变工作目录或权限需要断开后重新连接。文件输出仍不覆盖；失败或取消可能留下部分新文件，不提供文件事务回滚。

## 连接与错误边界

一个桌面连接拥有一个固定 control-cli 子进程；不会执行任意命令、Shell 或请求指定的 EXE。Rust 使用请求 ID 关联响应，前端用连接代次过滤旧连接的迟到响应。同一任务的旧响应也不能把取消中/终态退回运行中或清掉已有结果。协议错误、超时或退出不能冒充任务成功，也不自动重试启动。

单条请求等待上限15秒，最多8个等待响应的请求；请求4MiB、响应8MiB。Rust 在发送前拒绝非法操作、超大/过深请求时返回 desktop_request_rejected，保持有效连接；真正传输错误/超时则注销会话并请求取消，避免保留不可追踪任务。超限是控制通路的防护，不是音频任务的硬实时预算。

断开/窗口关闭时优先关闭 stdin，让 C++ 协作取消并清理；5秒未退出才强制结束本桌面实例自己创建的子进程。此时应按中断处理，不能保证所有输出文件完整，界面会保留警告；如果仍无法确认退出则拒绝重连/退出并提示。不要通过反复点击“运行”来恢复丢失的任务响应，先处理连接故障。

桌面默认不请求麦克风权限或播放音频。普通浏览器打开前端只作界面预览，不能连接 Tauri 后端；不使用假节点或假成功结果冒充实际运行。

## 代码边界

- `apps/desktop/src/`：React、样式和纯界面数据处理；不实现音频算法。
- `apps/desktop/src-tauri/src/`：Rust 窄命令、子进程生命周期、JSON 文件读取/创建。
- `apps/control_cli/`、`src/control_protocol.cpp`、`src/task_service.cpp`：C++ 控制协议和任务服务。
- Node/Graph/Executor 仍位于仓库 include/audioprocess 与 src 中，与前端框架无关。

更多任务状态与权限细节见 [受控任务接口](control-api.md)，实时限制见 [实时 Graph](realtime-audio.md)。本阶段只把已有能力呈现在界面，不代表 ASR、变声、降噪或任意实时图已经可用。

Tauri 的 sidecar 打包约定参见[官方文档](https://v2.tauri.app/zh-cn/develop/sidecar/)。

## 验证记录（2026-09-21）

- Node 内置测试12/12通过；TypeScript/Vite构建通过。
- Rust `cargo check --offline` 与7项测试通过，包含真实 Rust→control-cli 中文文本任务、旧会话失效、断连等待者拒绝、JSON 文件读写和本地拒绝后连接保持。
- 原 C++ Debug/Release 各19/19回归通过。
- 已使用实际 Windows 桌面应用验证：连接项目目录，发现12个真实节点，校验/启动默认文本图，显示已完成及中文结果；编辑使校验失效、重复JSON键提示正确。关闭窗口后检查没有本项目桌面或control-cli进程残留。全过程未开启设备/有声输出。
- 尚未实机触发15秒请求超时或5秒强杀；文件对话框的完整交互、界面中的持续音频/取消/拔插、有声试听和长时运行仍需后续人工体验。相关文件/状态/绑定逻辑已有自动测试，不冒充这些硬件与交互验收。
