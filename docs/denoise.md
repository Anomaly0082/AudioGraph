# RNNoise 离线语音降噪（P9）

`rnnoise_denoise` 封装官方 RNNoise v0.2 默认模型，不重新实现降噪算法。
它是一个已有 `ISyncNode` 契约下的 `Audio → Audio` 节点，Graph、Executor、控制协议和 AI 工具接口不变。

## 现在能做什么

- 整段离线处理48 kHz、单声道音频；现有文件输入/输出仅支持PCM16 WAV。
- 图内可放在 WAV 输入与输出之间，也可连接 Gain、Peak Meter 等整段节点。
- Registry公开端口、无参数约定及格式限制。桌面/MCP/内置AI沿用能力发现，不增加一个“降噪专用AI工具”。
- 默认模型本地CPU推理，不上传音频，不消耗模型API额度，不需要Python/GPU。

RNNoise主要针对语音背景噪声，不保证适用于音乐，也不能保证所有噪声都会被清除。实际语音可懂度和失真仍需要用目标录音试听评估。首版没有自定义模型、降噪强度参数、采样率转换、混音或实时/流式节点。

## 操作

桌面：连接音频目录 → 选择「语音降噪 · 含48 kHz单声道转换」→ 载入模板 → 把该目录的输入命名为 `input.wav`（或编辑路径）→ 校验 → 开始任务。输出为新文件 `denoised.wav`，不覆盖已有文件。P10模板增加了显式转单声道和重采样节点，可接受8–192 kHz单/双声道PCM16 WAV；降噪节点自身仍只接受48 kHz单声道，见 [格式适配](audio-format.md)。

也可以在内置AI的「输入 WAV 路径」填`input.wav`，描述「先适配格式，再用RNNoise降噪，输出到denoised.wav」，检查提案再确认。程序先查询真实格式再请求模型，但本阶段未付费调用真实模型验证其选用质量。

命令行：

```powershell
cmake --preset windows-msvc
cmake --build --preset debug --parallel
.\build\Debug\graph-demo.exe --describe-node rnnoise_denoise
.\build\Debug\graph-demo.exe --graph .\examples\graphs\denoise.json --validate
.\build\Debug\graph-demo.exe --graph .\examples\graphs\denoise.json
```

`graph-demo`相对路径基于JSON所在目录，因此该示例读取 `examples/graphs/input.wav`；桌面/MCP/control-cli则基于连接时的workspace，二者不要混淆。

**校验通过不代表已读取音频或检查文件格式。** 保持原来无I/O图校验的契约：非48 kHz、非单声道等实际内容问题在任务读取并执行到此节点时报告 `unsupported_audio_format`，结果包含节点/端口定位。不会静默转换。

## 数据与生命周期

内部仍使用归一化float音频。输入必须有限且在 `[-1,1]`，否则明确报错，不静默削波；前面Gain若放大过多，需要先降低增益。输出交给现有WAV节点编码及统计削波，降噪节点不额外限幅。

每次execute新建RNNoise状态，以480个采样一帧处理。对固定v0.2的960采样延迟做补偿；尾帧补零，再补两个静音帧排空，输出裁为输入长度。空音频仍为空，输入只读不被修改。每帧前后检查协作取消，取消不回滚其他节点已经生成的文件。

RNNoise内部按帧运算不等于平台已提供Streaming/Realtime节点：此适配器仍接收完整AudioClip，长文件内存消耗仍按整段计算，不承诺硬实时。

## 依赖与测试

源码和代码许可证固定在 `third_party/rnnoise`，当前集成目标为x86-64。模型文件因其独立许可待上游澄清，仅在本地准备并由Git忽略，不随源码提交；初次克隆需显式运行准备脚本。构建后模型嵌入静态库，运行时不下载权重。公开分发含权重的程序前必须确认授权，详见 [依赖说明](../third_party/rnnoise/README.integration.md)。

`denoise-node-tests`独立比较封装输出与原生RNNoise调用，覆盖短音频/尾帧、帧数、输入只读、状态隔离、格式/参数/幅值拒绝、预取消及真实WAV图处理。合成纯噪声RMS下降只证明该夹具的抑制行为，不作为真人语音质量或通用降噪指标。
