# Workflow v1：受限可执行程序

Workflow是由宿主程序执行的JSON文件，不是AI整体任务的状态机。外层AI可以编写、校验和运行文件；一次运行返回结果后，再自行决定是否修改、重跑或询问用户。程序内部不调用模型、不等待人工评价。

## 文件与调用

只在工具助手的Workflow模式开放：

- `workflow_validate({space:"user"|"ai",path:"gain.workflow.json"})`：读取一次文件，严格解析JSON并检查结构、引用作用域、工具白名单和预算；不运行步骤、不写文件、不启动音频处理。
- `workflow_run({space:"user"|"ai",path:"gain.workflow.json",inputs?:{...}})`：读取并固定本次程序，覆盖已声明的输入，然后执行。未知输入键拒绝，不重新读取执行中被修改的源文件。

文件最多64KiB，重复键、未知字段、未知步骤和未知工具拒绝。`space`只决定程序文件来源；所有内部文件操作仍按工具自己的空间规则，Graph只能在AI空间执行。不能借用户空间的Workflow文件获得用户文件修改权限。

## 最小结构

```json
{
  "schema_version": 1,
  "inputs": {"items": [1, 2, 3]},
  "steps": [
    {"id": "values", "type": "set", "value": {"$ref": "/inputs/items"}},
    {"id": "each", "type": "for_each", "items": {"$ref": "/steps/values"},
      "steps": [{"id": "value", "type": "set", "value": {"$ref": "/item"}}]}
  ],
  "outputs": {"values": {"$ref": "/steps/each"}}
}
```

顶层只允许`schema_version`、`inputs`、`steps`、`outputs`、可选`limits`；版本必须为整数1。输入默认值是普通JSON数据，不会把输入字符串当代码。`outputs`为最后求值的对象。

### 值与引用

对象、数组和标量按原JSON结构构造；只有恰好一个键的`{"$ref":"/inputs/name"}`是引用，使用JSON Pointer路径。可引用：

- `/inputs/...`：本次输入；调用参数只能覆盖声明过的键。
- `/steps/<id>/...`：已经完成的步骤结果，同时承担不可变变量绑定；不支持前向引用。
- `/item/...`、`/index`：当前for_each元素与从0开始的序号，仅循环体内有效。

引用得到的是数据，不把被引用内容再次解释成表达式。需保留字面量`$ref`对象时用`{"$literal":任意JSON}`。不支持字符串插值、算术代码、eval或任意函数；文件名可作为输入数组中各项的字段传入。

### 步骤

每个步骤必须有独立`id`（字母开头，后续字母、数字、下划线或短横线，最多64字符）。同一可见作用域不能重名；循环体和条件分支是子作用域，不泄漏其变量。

- `set`：`{"id":"x","type":"set","value":表达式}`，求值后保存至`steps.x`。
- `call`：`{"id":"read","type":"call","tool":"file_read_text","args":{"space":"ai","path":"note.txt"}}`。参数递归求值，结果是工具返回的数据对象，保存至`steps.read`；工具失败就终止程序，不自动重试。
- `for_each`：`{"id":"batch","type":"for_each","items":表达式,"steps":[...]}`。items必须为数组，顺序处理，结果为每次迭代的局部步骤结果对象数组。单层最多16个元素；外层已完成的步骤可读，内层绑定在迭代结束后丢弃。
- `if`：`{"id":"choice","type":"if","condition":{"op":"lt","left":表达式,"right":表达式},"then":[...],"else":[...]}`。else可省略。仅执行所选分支，结果为`{"branch":"then"|"else","steps":{局部结果}}`。op支持eq/ne/lt/lte/gt/gte；顺序比较要求有限数值，eq/ne按JSON值比较。没有while、跳转或递归。

`call`白名单固定为本轮已有基础工具：workspace_list、file_read_text、file_write_text、file_delete、file_copy_to_ai、file_export、audio_inspect、nodes_list、graph_validate、graph_run。不能调用workflow_run/workflow_validate、模型接口或宿主设置命令。

Graph可在call的args中写成带值引用的JSON结构，也可整体作为输入传入。首版不提供任意JSON字符串解析或通用对象补丁指令；已有Graph内容可由外层AI读取后放入输入/调用模板。

## 预算、停止与结果

limits默认：max_steps=128、max_tool_calls=32、max_graph_runs=8、timeout_ms=120000；最高分别为256、64、16、300000。控制结构与每次实际执行的子步骤都计数，循环为空也计算其控制步骤。程序嵌套最多4层，JSON/表达式深度最多32。运行状态/结果序列化总量限制1MiB，单个求值结果最多256KiB。

每个步骤和工具调用前检查取消、时限与预算，Graph运行时沿用同一取消信号，并以剩余Workflow时限收紧Graph自身60秒时限。不能通过多层循环或嵌套工作流绕过限制。取消不意味着回滚已写文件，清理完成前不返回“已停止”。

返回结构化报告：`schema_version`、`state`（succeeded/failed/cancelled/limited）、`outputs`、`step_results`、`trace`、计数及适用的`error`。错误包含code/message/step_path，trace可定位循环的具体迭代。失败时outputs为空，保留已完成步骤及工具信息，不把中途结果伪装为最终成功。源文件space/path及SHA256记录在工具返回中。

结果容量异常是上述保留规则的例外：若加入元数据后整个报告仍超限，则明确返回limited与result_limit，并说明部分结果详情因超限未包含；不会声称成功或回滚已完成的操作。

校验只保证程序静态契约；运行时输入类型、文件存在性、Graph合法性和底层资源仍由工具再次检查。输入、返回值和结果超限必须显式失败，不静默截断成成功。没有后台恢复、并行步骤、文件事务或模型自动修复。

此处预算是程序执行上限，不代表模型上下文容量。外层AI请求仍受128KiB限制；若完整工具结果使本轮请求超限，会明确停止，不自动丢弃当前结果或继续。首版应返回精简outputs并限制批次大小；结果分页/持久化索引尚未实现。

首版变量是不可变的步骤结果。for_each遍历有限列表，不能读取前一次迭代的局部变量或更新累加器；自适应收敛循环暂不支持。

## 首个验收场景

示例`examples/workflows/gain-comparison.workflow.json`：从用户空间复制一次PCM16 WAV到AI空间 → 遍历三组gain_db与独立输出路径 → 分别执行Graph → 汇总每次Graph真实结果并返回。原输入不改，输出不覆盖；校验不会复制文件或生成音频。重复运行示例需换一组新路径，否则明确失败。

测试另覆盖set与引用、条件两分支、作用域、未知工具/递归拒绝、变量类型错误、循环和时间预算、停止后的步骤不执行、失败保留前面产物，以及Graph模式伪造Workflow工具被拒绝。开发使用本地mock/真实C++小WAV，不调用用户付费API。

本轮自动验证：前端82项、Rust79项通过，付费实测1项忽略；独立审核另补嵌套作用域、失败结果保留与Graph次数预算测试。桌面与真实供应商交互由用户验收；不将自动测试等同于模型稳定编写Workflow的质量保证。
