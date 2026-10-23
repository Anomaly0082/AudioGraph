# Workflow v1：受限可执行程序

Workflow是由宿主程序执行的JSON文件，不是AI整体任务的状态机。外层AI可以编写、校验和运行文件；一次运行返回结果后，再自行决定是否修改、重跑或询问用户。程序内部不调用模型、不等待人工评价。

## 文件与调用

AI工具只在工具助手的Workflow模式开放；人工也可以从“编辑器 → Workflow”直接校验和运行，不需要模型API：

- `workflow_validate({space:"user"|"ai",path:"gain.workflow.json"})`：读取一次文件，严格解析JSON并检查结构、引用作用域、工具白名单和预算；不运行步骤、不写文件、不启动音频处理。
- `workflow_run({space:"user"|"ai",path:"gain.workflow.json",inputs?:{...}})`：读取并固定本次程序，覆盖已声明的输入，然后执行。未知输入键拒绝，不重新读取执行中被修改的源文件。

文件最多64KiB，重复键、未知字段、未知步骤和未知工具拒绝。`space`只决定程序文件来源；所有内部文件操作仍按工具自己的空间规则，Graph只能在AI空间执行。不能借用户空间的Workflow文件获得用户文件修改权限。

## 桌面手动编辑

编辑器提供Graph/Workflow两页签，各自保留草稿。Workflow使用JSON文本，文件工作区可选用户或AI。打开可以读取语法错误的文本供修复；保存和另存前都必须通过后端静态校验。“保存”按载入时的内容指纹检查后写回关联文件；无来源时按新文件保存。“另存为”只新建、不覆盖，用户空间弹出新文件选择框，AI空间填写新的相对路径；新建目录沿用AI目录3层与用户一级目录的限制。上方打开/另存路径不会自动改变“保存”的原文件关联。外部修改冲突会保留草稿并提示重新打开或另存，不提供强制覆盖；AI文件工具权限未扩大。

编辑后点击“校验”，只有当前工作区和完全相同的草稿仍有效时才可点击“运行 Workflow”。实际运行直接传入固定文本快照，后端重新验证，不重新读取已保存文件，也不调用模型。运行中编辑下一份草稿不会改动当前执行；页面切换不会停止任务，停止按钮和关窗取消等待私有音频后端清理。连接失效时会取消仍持有的旧请求；迟到结果不写入新工作区。

手动Workflow及其内部Graph均标记`origin:manual`，但内部Graph仍在AI文件空间执行，记录中的`configuration.file_space:ai`明确这一点。人工直接运行Graph的路径规则仍是用户工作区。结果进入既有运行记录，页面仅显示状态及“查看运行记录”入口，不新增独立结果系统。

AI成功写出的完整Workflow文本可点击“载入 Workflow 编辑器”，确认后只替换草稿，必须另行校验与点击运行。该入口载入生成时的内容快照，不保证磁盘上的同名文件之后没有改变；不接受已省略详情的事件作为完整配置。

人工编辑器增量验证：前端120项、Rust138项通过，付费API实测1项忽略；实现代理交叉检查完成。覆盖纯校验无写入、坏JSON载入修复、另存无覆盖/路径限制、运行快照、取消与关窗共享清理、真实C++manual父子记录及AI执行空间、AI只载入不执行。未操作真实窗口；手动验收可先将`examples/workflows/condition.workflow.json`粘入Workflow页签，校验运行后查看记录，再用实际音频配置验证。

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
- `/item/...`、`/index`：当前for_each元素与从0开始的序号，在循环体内有效；也可在`filter.condition`、`select.by`中临时指向正在检查的元素和序号。

引用得到的是数据，不把被引用内容再次解释成表达式。需保留字面量`$ref`对象时用`{"$literal":任意JSON}`。不支持内联算术代码、字符串模板插值、eval或任意函数；计算和拼接通过下面的独立步骤完成，再引用其结果。没有新增特殊表达式键，旧配置中的普通对象和字符串含义保持不变。

### 步骤

每个步骤必须有独立`id`（字母开头，后续字母、数字、下划线或短横线，最多64字符）。同一可见作用域不能重名；循环体和条件分支是子作用域，不泄漏其变量。

- `set`：`{"id":"x","type":"set","value":表达式}`，求值后保存至`steps.x`。
- `compute`：`{"id":"delta","type":"compute","op":"sub","args":[表达式,表达式]}`，输出一个数值。`add/sub/mul/div`必须恰好2项，`abs`恰好1项。
- `concat`：`{"id":"filename","type":"concat","parts":["result-",{"$ref":"/index"},".wav"]}`，输出一个字符串；这个例子需放在循环体内。parts必须是固定数组，最多64项，空数组输出空字符串；每项可用既有数据引用。
- `filter`：`{"id":"eligible","type":"filter","items":表达式,"condition":{"op":"lte","left":{"$ref":"/item/clipped"},"right":0}}`，输出按原顺序保留的完整元素数组，条件结构与比较规则同`if`。
- `select`：`{"id":"best","type":"select","items":表达式,"by":{"$ref":"/item/distance"},"order":"min"}`，按数值指标选最小/最大项；order必须为`min`或`max`，返回结构见下节。
- `call`：`{"id":"read","type":"call","tool":"file_read_text","args":{"space":"ai","path":"note.txt"}}`。参数递归求值，结果是工具返回的数据对象，保存至`steps.read`；工具失败就终止程序，不自动重试。
- `for_each`：`{"id":"batch","type":"for_each","items":表达式,"steps":[...]}`。items必须为数组，顺序处理，结果为每次迭代的局部步骤结果对象数组。单层最多16个元素；外层已完成的步骤可读，内层绑定在迭代结束后丢弃。
- `if`：`{"id":"choice","type":"if","condition":{"op":"lt","left":表达式,"right":表达式},"then":[...],"else":[...]}`。else可省略。仅执行所选分支，结果为`{"branch":"then"|"else","steps":{局部结果}}`。op支持eq/ne/lt/lte/gt/gte；顺序比较要求有限数值，eq/ne按JSON值比较。没有while、跳转或递归。

`call`白名单为基础工具：workspace_list、directory_create、file_read_text、file_write_text、file_delete、file_copy_to_ai、file_export、audio_inspect、audio_convert、nodes_list、graph_validate、graph_run。directory_create仅创建AI空间目录，最多3层；file_export可自动创建用户空间下一级结果目录但不覆盖已有文件。audio_convert支持output_format为wav（默认PCM16）/flac/mp3/m4a，在AI空间新建目标，不覆盖、不删除源。后续Graph或audio_inspect仍需选wav；压缩格式可用作最终交付，返回path供导出。不能调用workflow_run/workflow_validate、历史查询工具、模型接口或宿主设置命令。

Graph可在call的args中写成带值引用的JSON结构，也可整体作为输入传入。首版不提供任意JSON字符串解析或通用对象补丁指令；已有Graph内容可由外层AI读取后放入输入/调用模板。

### 计算与拼接规则

`compute`只接受求值后的数值，不把数字字符串、布尔值或null隐式转成数字。运算采用有限的IEEE 754双精度浮点（f64），不是任意精度整数/十进制计算：小数计算可能有舍入误差。JSON整数输入超出`[-9007199254740991,9007199254740991]`返回`numeric_precision`，避免大整数转浮点时悄悄丢失信息；浮点输入与结果仍按f64处理。除以正零或负零返回`division_by_zero`，结果溢出为无穷等非有限数时返回`numeric_overflow`；不输出NaN/null冒充结果。

计算结果始终为浮点JSON数值。既有`eq/ne`维持JSON值精确比较：整数`5`与浮点`5.0`不相等；`lt/lte/gt/gte`会按数值比较。判断计算结果是否接近目标时，推荐`sub → abs → lte 容差`，不要依赖浮点精确相等。本次不修改旧比较规则。

`concat`只接受字符串和数字，按顺序直接连接，不自动添加分隔符；数字使用JSON数字文本表示，因此整数`1`和计算得到的浮点`1.0`可能拼出不同文字。布尔、null、数组和对象返回`invalid_type`，没有模板执行或字符串转代码。拼接结果按JSON编码字节数（含引号、UTF-8及控制字符转义）受256KiB限制，追加下一项之前检查，不先制造超限大字符串。表达式和引用仍遵守既有深度、作用域与容量限制。

这两个步骤只计算数据，每次都计入`max_steps`并写入trace，不计入工具/Graph调用次数；不自动读写文件或请求模型。构造出的路径依然由文件工具检查，不能绕过目录深度、空间、越界、同名覆盖限制，也不保证重跑时文件名仍唯一。静态校验检查结构、操作符、参数个数和引用；类型、除零、结果容量等在运行时检查。失败会终止后续步骤，保留之前已完成结果；取消和超时机制不变。

例如先计算差值，再求绝对值，最后可以由后续`if`比较阈值：

```json
{
  "schema_version": 1,
  "inputs": {"actual": 0.2, "target": 0.25},
  "steps": [
    {"id": "delta", "type": "compute", "op": "sub", "args": [{"$ref": "/inputs/actual"}, {"$ref": "/inputs/target"}]},
    {"id": "distance", "type": "compute", "op": "abs", "args": [{"$ref": "/steps/delta"}]}
  ],
  "outputs": {"distance": {"$ref": "/steps/distance"}}
}
```

文件示例`examples/workflows/computed-gain.workflow.json`：复制一次输入 → 用序号计算三组增益并拼接输出路径 → 执行Graph → 读取峰值、计算与目标的距离 → 导出到一个用户子目录。运行前需把`inputs.source`改为实际WAV文件，并为每批选择新的`inputs.folder`。该示例仍导出所有候选；仅导出所选项的版本见下节。

计算/拼接增量验证：后端222项通过、4项需外部条件的测试忽略；独立审查完成。覆盖操作符、参数个数、运行时类型、整数转换精度、除零/溢出、中文和转义字符串恰好256KiB边界、循环/条件作用域、步骤预算、取消/超时、旧JSON精确比较与字面量语义，以及通过实际C++后端执行示例的三组增益/峰值差距/文件导出。未调用真实模型API，也未访问音频设备；模型实际编写效果仍由用户体验验证。

### 过滤与选优规则

两者的`items`必须求值为数组，最多16项；不是数组返回`invalid_type`，超出16项返回`item_limit`，不截断、不抽样。`items`先在外层环境求值，逐项检查期间才临时设置`/item`和`/index`，结束或报错都恢复外层绑定。因此在外层循环中可用`items:{"$ref":"/item/results"}`读取子数组；若谓词还需要外层元素/序号，请先用`set`保存为步骤结果再引用。

`filter`不重建或修改元素，只保留满足条件的完整项及原顺序；空数组或全不满足时返回`[]`。条件引用不存在的指标会报错；数值顺序比较遇到字符串、null等错误类型会报错，不当作普通淘汰。`eq/ne`仍按JSON值比较，可用于字符串/布尔等条件；检查削波数量推荐`lte 0`，避免整数`0`与浮点`0.0`的精确比较区别。多个条件可串联多个filter步骤。

`select`的`by`在每项作用域内求值，只允许有限数值，并沿用compute的安全整数转换约束。必须检查所有候选，末项指标错误也会失败，不能忽略错误后宣布选出最佳项。按数值比较，完全相同的指标取最早项，不额外排序：

```json
{"found":true,"item":{"gain_db":0,"path":"trial-1.wav","distance":0.1},"index":1,"score":0.1}
```

`item`保留完整记录，`score`保留该项原始数字值。`index`是select输入数组的从0开始下标；如果输入经过filter，它就是过滤后数组的下标，不是原候选序号。需要追溯原序号时在候选记录里保留`source_index`。

空数组返回`{"found":false,"item":null,"index":null,"score":null}`，这属于成功执行但没有选中项，不是工具失败。必须先用`if`检查`/steps/best/found`，再读取`/steps/best/item/...`或导出；没有选中时读取item内部字段仍会报错。即使合法原项本身是null，也可通过found区分“选中了null”和“没有选中”。

每个filter/select计一次步骤预算，不计工具或Graph调用次数；每项检查取消/时限，保留单值256KiB和总状态1MiB限制，select结果额外封装超限也会失败。它们不自动导出、删除候选文件、运行音频或调用模型，只按配置的判据处理数据。旧表达式、文件权限和失败后的部分结果保留规则不变。

完整示例`examples/workflows/select-gain.workflow.json`：多组增益处理 → 计算与目标峰值的距离 → 过滤削波候选 → 选择距离最小项 → 仅导出选中音频。所有候选仍留在AI空间供查看，用户空间不会被批量中间文件填满。若所有候选都削波，报告成功且found=false，不产生导出。峰值距离只是示例判据，不等同于音质评分，也不提供自动收敛或无限搜索。

过滤/选优增量验证：后端233项通过、4项需外部条件的测试忽略；独立审查完成。覆盖空结果、min/max、并列首项、完整记录保留、后续坏指标仍报错、嵌套与失败时作用域恢复、16项上限、单值/状态容量、步骤预算和取消/超时。实际C++音频测试确认三组候选中排除削波项且仅导出一个选中结果；仅有削波候选时不导出，源音频不变。未调用真实模型API；应用内生成配置的模型表现仍需用户体验验证。

## 预算、停止与结果

limits默认：max_steps=128、max_tool_calls=32、max_graph_runs=8、timeout_ms=120000；最高分别为256、64、16、300000。控制结构与每次实际执行的子步骤都计数，循环为空也计算其控制步骤。程序嵌套最多4层，JSON/表达式深度最多32。运行状态/结果序列化总量限制1MiB，单个求值结果最多256KiB。

每个步骤和工具调用前检查取消、时限与预算，Graph运行时沿用同一取消信号，并以剩余Workflow时限收紧Graph自身60秒时限。不能通过多层循环或嵌套工作流绕过限制。取消不意味着回滚已写文件，清理完成前不返回“已停止”。

返回结构化报告：`schema_version`、`state`（succeeded/failed/cancelled/limited）、`outputs`、`step_results`、`trace`、计数及适用的`error`。错误包含code/message/step_path，trace可定位循环的具体迭代。失败时outputs为空，保留已完成步骤及工具信息，不把中途结果伪装为最终成功。源文件space/path及SHA256记录在工具返回中。

结果容量异常是上述保留规则的例外：若加入元数据后整个报告仍超限，则明确返回limited与result_limit，并说明部分结果详情因超限未包含；不会声称成功或回滚已完成的操作。

校验只保证程序静态契约；运行时输入类型、文件存在性、Graph合法性和底层资源仍由工具再次检查。输入、返回值和结果超限必须显式失败，不静默截断成成功。没有后台恢复、并行步骤、文件事务或模型自动修复。

此处预算是程序执行上限，不代表模型上下文容量。外层AI请求仍受128KiB限制。Graph/Workflow执行回执超过12KiB时，仅模型上下文改用明确标注`complete:false`的摘要，保留成功/失败、状态、记录ID、错误及保存警告；本地工具详情和运行库保留原返回报告（仍受各自存储上限）。模型可通过`runs_read`按记录ID及JSON Pointer读取结果；保存警告意味着记录可能不完整。多个消息累计仍可能达到128KiB，此时明确停止，不自动无限续跑。

注意两层返回结构不同：外层AI工具协议是`{ok,data}`，Workflow内部call只保存原始data，不带这一层包装。例如Graph导出值为`/steps/render/result/outputs/peak/value`；循环第一个元素内步骤为`/steps/batch/0/child`；条件分支内步骤为`/steps/choice/steps/child`。这些说明同时提供给模型，减少依靠试错猜路径。

首版变量是不可变的步骤结果。for_each遍历有限列表，不能读取前一次迭代的局部变量或更新累加器；自适应收敛循环暂不支持。

## 首个验收场景

示例`examples/workflows/gain-comparison.workflow.json`：从用户空间复制一次PCM16 WAV到AI空间 → 遍历三组gain_db与独立输出路径 → 分别执行Graph → 汇总每次Graph真实结果并返回。原输入不改，输出不覆盖；校验不会复制文件或生成音频。重复运行示例需换一组新路径，否则明确失败。

测试另覆盖set与引用、条件两分支、作用域、未知工具/递归拒绝、变量类型错误、循环和时间预算、停止后的步骤不执行、失败保留前面产物，以及Graph模式伪造Workflow工具被拒绝。开发使用本地mock/真实C++小WAV，不调用用户付费API。

本轮自动验证：前端82项、Rust79项通过，付费实测1项忽略；独立审核另补嵌套作用域、失败结果保留与Graph次数预算测试。桌面与真实供应商交互由用户验收；不将自动测试等同于模型稳定编写Workflow的质量保证。
