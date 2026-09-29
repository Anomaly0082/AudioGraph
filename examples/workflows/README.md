# Workflow v1示例

- `gain-comparison.workflow.json`：独立复制用户WAV，执行三组增益，返回文件路径、峰值与削波数。文件执行不调用模型、不等待用户。
- `condition.workflow.json`：纯数据绑定与条件分支，不运行音频、不写文件。

在工具助手选择Workflow模式，可先让AI调用`workflow_validate`读取用户空间中的示例文件，再调用`workflow_run`。若工作区为仓库根目录，path为`examples/workflows/condition.workflow.json`；运行增益示例前把PCM16 WAV放入工作区并通过`inputs.source`指定其相对路径。

示例复制目标与输出位于AI工作区，用户原始文件不变。相同路径重复执行会因拒绝覆盖而失败；再次运行需用inputs覆盖copy_path与candidates的output，选择全新路径。交付文件由外层AI显式调用file_export。

完整语法、返回值与限制见[Workflow v1契约](../../docs/workflow-v1.md)。本目录不是Python或Shell脚本入口。
