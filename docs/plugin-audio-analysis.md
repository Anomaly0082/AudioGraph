# 音频分析插件

插件ID：`org.audiograph.analysis`。节点类型：`org.audiograph.analysis.audio_metrics_v1`，界面显示名 **Audio Metrics**。使用现有`ag.whole_sync/1`公开接口，独立链接SDK，不依赖主程序内部计算模块，也不依赖模型、网络或额外算法库。

## 能力和定义

只接受整段`Audio`输入，输出4个`Number`，不修改音频、不写文件、不提供音频直通端口。当前用于离线Graph；不是实时节点，也不能直接混入离线分块streaming链。

| 输出 | 定义 |
| --- | --- |
| `rms` | 全部声道所有采样的均方根，线性幅度；满幅参考为1。 |
| `rms_dbfs` | `20 × log10(rms)`，最小返回−120 dBFS；全零也返回−120，原始`rms=0`仍可区分。超幅信号可大于0 dBFS。 |
| `clipping_ratio` | 超出PCM16范围`[-1,32767/32768]`的采样数除以总采样数，所有声道共同统计，范围0..1。 |
| `silence_ratio` | 低电平窗口所占的实际帧数除以总帧数，范围0..1。阈值可调，规则如下。 |

参数`silence_threshold_db`默认−60，范围−120..0 dB。按连续标称20 ms窗口计算所有声道采样的RMS；窗口帧数为`max(1,(sample_rate+25)/50)`（整数除法），RMS小于等于阈值对应线性幅度时计为低电平。尾部不足整窗时，用实际帧数加权，不把短尾窗当成一整窗。多声道是共同均方统计，不是“所有声道分别判静音”。

空音频拒绝，避免输出没有定义的比例；非有限采样拒绝。有限超幅样本可以分析，不被裁剪；输出因此能在WAV导出前预判削波。PCM16正端点不对称：`+1.0`会计入削波，`−1.0`不会，与现有`wav_output`一致。若要分析已导出/限幅后的文件，采样已被裁剪，无法据此恢复之前丢失的削波信息。

这里的静音比例是**低电平时长的近似指标，不是语音活动识别**。RMS也不是LUFS响度；这些数值不能代替听感评价，不应把单个数值更小或更大直接解释为音质更好。

两个比例都使用0..1表示，例如0.5代表50%，不是数值50。

## 使用

1. 在“设置 → 节点插件”安装完整的`build/audio-analysis-sdk-standalone/plugin-build/package/Release`目录，重启程序。
2. 将`examples/graphs/plugin-audio-analysis.json`复制到工作区，修改其中`input.wav`为实际输入，使用离线模式校验并运行。四项指标显示在既有运行记录中，不会产生新音频文件。
3. 需要同时处理/导出音频时，从前一个音频节点分两路：一路连接分析节点，另一路连接`wav_output`。分析节点输出的Number可以作为Graph exports供Workflow或AI使用。

`examples/workflows/analysis-selection.workflow.json`演示：复制输入到AI工作区 → 测试三组增益 → 使用插件分析 → 排除削波和低电平占比过高的结果 → 选出RMS最接近目标的一组 → 只导出选中音频。`target_rms`是线性幅度，`max_silence_ratio`是0..1比例；每次使用新的`folder`避免覆盖。全部候选被过滤时返回未选中且不导出，不假装达成目标。

安装后，现有节点发现接口会向AI提供插件的参数、端口及计算定义，不增加新的专用工具或实验页面。没有自动运行或自动上传音频。

## 构建与验证

源码在`examples/plugins/audio-analysis`，可复制到宿主仓库外，用已安装的`AudioGraphPluginSDK`独立编译；见目录README。Windows x64/MSVC，C++20，静态CRT。schema 2插件包只包含manifest、DLL和节点描述，成员逐项SHA256校验。

在仓库中可执行：

```powershell
cmake --build build --config Debug --target audio-analysis-host-tests
ctest --test-dir build -C Debug -R "^audio-analysis-" --output-on-failure
cmake -DSOURCE_ROOT=C:/AAAProject/AudioProcess -DTEST_ROOT=C:/AAAProject/AudioProcess/build/audio-analysis-sdk-standalone -DGENERATOR="Visual Studio 17 2022" -DPLATFORM=x64 -DCONFIG=Release -P tests/audio_analysis_sdk_standalone.cmake
```

自动验证使用合成数据，覆盖RMS参考值、多声道、采样率、阈值边界、尾窗权重、PCM16端点、空数据/NaN/溢出拒绝、预算、取消/超时、宿主回调拒绝和异常、实例生命周期、真实Graph、安装及Workflow使用。原生插件是可信代码而非沙箱；指针校验只能拒绝空值、错位和地址跨度溢出，不能证明任意指针指向可访问内存。

本轮独立审查及自动验证完成：C++31项回归通过，后端236项通过、6项需外部条件的用例忽略；独立SDK的Debug/Release构建及真实宿主执行均通过。临时工作区安装、AI目录发现、三组RMS选优、实际削波候选淘汰、全部低电平占比不合格时不导出均已验证。没有调用模型API、访问设备或安装到用户实际插件目录；界面安装体验由用户验收。

### 后续真实模型验收

用户另行明确批准后，在全新隔离工作区临时安装Release插件，使用本机API配置进行一次真实模型验收。没有向任务提供插件名称、typeId或示例配置，也未修改AI系统提示。模型通过nodes_list自行发现能力并编写、校验、运行Workflow。

合成输入包含整窗零电平、低幅度和高幅度片段。目标为线性RMS最接近0.55、PCM16无削波，−60 dBFS阈值下低电平时长不超过30%；候选−6/0/+3/+12 dB。实测模型正确使用分析节点：前两组因40%低电平占比被排除，+12因60%削波比例被排除，最终选择+3 dB（RMS 0.5470736799、削波0、低电平占比20%），只导出选中音频。

10次模型请求、15次外层工具调用均成功；内部Graph、compute/filter/select记录与原始音频理论值独立比对通过，交付WAV元数据、RMS和哈希与所选产物一致，输入未改。模型没有直接绕过Workflow执行Graph，也没有上传音频或改动用户实际插件目录。

证据位于Git忽略的`out/live-analysis-14704-1791467337651279800/`，report.json为原始通过报告，executed-workflow-*.json为实际配置，result.wav为合成输出。此前一次准备尝试因测试安装路径未规范化而在API请求前被拒绝，修正测试路径后进行上述唯一真实模型运行；没有为取得成功反复请求模型。该结果证明本插件可通过现有节点描述被模型使用，不代表任意第三方插件或主观音质评判都已保证。
