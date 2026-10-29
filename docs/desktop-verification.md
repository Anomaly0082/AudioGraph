# 离线桌面验证

在仓库中用 PowerShell 7 执行：

```powershell
pwsh -NoProfile -File scripts/verify-desktop.ps1 -List
pwsh -NoProfile -File scripts/verify-desktop.ps1
```

默认依次运行桌面前端 `npm test`、`npm run build`、Rust `cargo test --offline`，以及构建元信息的纯数据测试。首次运行前需已安装 npm 依赖、缓存 Cargo 依赖，并构建本地测试所需的 control-cli/FFmpeg。入口不安装或下载依赖，不构建或运行最终桌面应用。避免与其他 Cargo/Tauri 构建同时执行，以免争用构建缓存。

Windows 需已有 VS2022 C++ 工具，且当前终端能找到 `cl.exe` 和 `link.exe`。请从 **Developer PowerShell for VS 2022 (x64)** 进入 PowerShell 7 后执行。缺少开发环境时脚本会给出提示并停止，不自动修改 `PATH`、`INCLUDE` 或 `LIB`。

可选的浏览器检查仅使用已经存在的 Playwright 模块入口文件和 Chromium：

```powershell
pwsh -NoProfile -File scripts/verify-desktop.ps1 -Headless -PlaywrightModule 'C:/tools/playwright/index.js' -Chromium 'C:/tools/chromium/chrome.exe'
```

也可事先设置 `AUDIOGRAPH_PLAYWRIGHT_MODULE` 与 `AUDIOGRAPH_CHROMIUM`。缺少文件时会在任何测试开始前失败，不会自动安装浏览器。此模式追加六个无头脚本，验证工作区文件交互（含复制路径）、对话输入性能、工作台布局、会话删除、异常恢复及节点编辑撤销/复制粘贴；使用模拟的原生边界，不启动WebView2，不写真实剪贴板或删除真实会话。`-List -Headless`只打印包含这六步的固定JSON计划，不检查工具或创建输出。

异常恢复检查覆盖：请求与历史读取同时失败、取消失败后重新取消、收到回复后历史读取失败、旧历史/AI响应晚到、未发送或已恢复草稿在同目录新会话连接后保留，以及切换目录时草稿隔离。保留范围是本次程序进程内；不会自动重新发送请求。在途请求遇到工作区断开后仍以保存的会话记录为准，不承诺恢复尚未保存的模型回复。

每次实际验证的独立日志位于 `build/desktop-verification/<时间-guid>/`。`summary.json` 包含步骤状态、退出码、耗时、可选 Git HEAD 和工作树是否有未提交变更；不保存环境或 API 配置。`git_dirty:true` 的结果不能冒充该HEAD的干净检出验证。第一个失败步骤会终止后续执行，日志保留，退出码非零。工作目录及临时修改的 Cargo/Playwright 环境变量在结束时恢复。

入口不传递 `--ignored`，不运行原生 WebView、设备、收费 API 或回收站测试。原生测试仍需单独明确授权，不属于此入口。只验证入口自身的计划和前置失败行为可运行：

```powershell
pwsh -NoProfile -File scripts/test-verify-desktop-plan.ps1
```

## 2026-10-25 稳定性检查

- 已整理提交 `b7f6fa5`；旧 ASR 插件改动留在工作区，不包含在该提交中。测试工作树仍含这些未提交改动，不等同于干净检出验证。
- 完整检查通过：前端 199 项、Rust 302 项（14 项按原配置忽略）、构建元信息测试及四个原有无头脚本。日志：`build/desktop-verification/20261025-131936-d60fc9eaa0df4d329c709f2574ea5eb2/summary.json`。
- 新发现并修复两处输入恢复问题：请求及历史读取同时失败时未恢复输入；后端断开后同目录重连清空未发送草稿。两处均先复现断言失败，再修复验证。
- 修复后前端 199 项、会话删除无头回归、验证入口计划及新增 7 组会话恢复场景通过。新增恢复脚本已加入 `-Headless` 的后续运行计划。
- 文件操作检查包含试听释放后重命名/删除、转换取消与重试、输出冲突及切换工作区后旧响应隔离；未调用真实剪贴板、回收站、音频设备、收费 API 或原生 WebView2。
