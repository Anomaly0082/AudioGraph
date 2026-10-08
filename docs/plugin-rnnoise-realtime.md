# RNNoise 实时降噪插件（本机实验）

插件 `org.audiograph.rnnoise` 提供 `org.audiograph.rnnoise.realtime_v1`，显示名见节点目录。它使用既有 `ag.realtime_audio/1` 接口和公开 SDK，算法与模型由插件自行携带，不链接 AudioGraph 核心，不修改 Node/Executor 接口，也不增加专用前端页面。

## 当前范围

- 输入与输出均为 48 kHz、单声道、归一化 float32 音频流；不自动处理其他采样率或声道数。
- 无“降噪强度”参数，复用项目固定 RNNoise v0.2 默认模型。
- 支持不同长度的设备音频块；每实例保留独立的分帧缓冲与算法状态。停止后再运行创建新状态。
- 输入必须是有限的 `[-1,1]` 样本。需要增益时优先放在降噪后；超范围失败不会静默限幅或绕过降噪。非有限输入也会失败，沿用宿主静音并停止的处理。
- 实时回调只处理已准备的内存；分配和初始化发生在设备启动前，不在回调中读文件、联网或输出日志。

插件每凑齐 480 个采样（10 ms）处理一帧，下一帧开始输出结果。额外缓存延迟为 480 个采样，加上固定 RNNoise v0.2 的 960 个采样算法延迟，总共约 **30 ms**，不包含设备、系统和虚拟声卡缓冲。实时不能像离线处理那样丢掉开头、读取未来来补偿延迟。停止时不追加音频排空；最后尚未输出的缓冲/算法尾音被丢弃，不能把此接口当作保留文件尾部的离线导出器。

## 本机构建与测试

已有校验过的模型位于 `third_party/rnnoise`；构建不下载新内容。源码位置为 `examples/plugins/rnnoise-realtime`，独立使用已安装的 SDK 和显式提供的 `AG_RNNOISE_SOURCE_DIR`，具体命令见该目录 README。

从仓库根目录运行下列测试（已配置并构建主工程测试程序）：

```powershell
ctest --test-dir build -C Debug -R rnnoise-realtime --output-on-failure
cmake -DSOURCE_ROOT=C:/AAAProject/AudioProcess -DTEST_ROOT=C:/AAAProject/AudioProcess/build/rnnoise-realtime-sdk-standalone -DGENERATOR="Visual Studio 17 2022" -DPLATFORM=x64 -DCONFIG=Release -P tests/rnnoise_realtime_sdk_standalone.cmake
```

Release 包位于 `build/rnnoise-realtime-sdk-standalone/plugin-build/package/Release`。测试使用合成音频，不访问麦克风：直接 RNNoise 参考比对、不同分块一致性、静音、状态隔离/重建、非法参数/音频、真实宿主发现/执行/失败静音。性能计时只是本机观察，不构成实时期限或主观降噪质量保证。

本次本机 Release 合成测试（100 次非静音 480 采样回调）记录：中位数 0.266 ms、P95 0.325 ms、最大 0.352 ms；这是每帧算法调用耗时，不是端到端延迟，不含其他应用争用与设备缓冲。无分配要求经实现/固定依赖代码审查，本轮未做 malloc 或系统分配的动态拦截监测。用户已确认人工验证完成；未另行采集设备型号、端到端延迟或主观效果评分，不据此承诺其他设备上的表现。

## 安装与人工验收

1. 设置 → 节点插件 → 安装，选择完整的 Release 包目录，然后重启软件。
2. 把 `examples/graphs/plugin-realtime-denoise.json` 复制到自己的工作区，在 Graph 编辑器打开，选择实时模式。
3. 选择并应用实际输入/输出设备；允许设备访问，先运行静音 probe，确认没有节点错误。
4. 戴耳机、低音量，允许有声输出并手动关闭 probe 后运行。样例在降噪后使用内置 Gain −6 dB，避免试听音量过高。
5. 对比同一设备和增益下“不含降噪节点”的图，分别试听环境噪声、说话、停顿；检查吞字、断音、延迟。停止再启动应正常，无上次缓冲音频残留。
6. 最后把输出切到已验证过的虚拟音频设备，在会议/录音程序中选择其对应输入，验证跨软件链路。检查运行记录中的错误和欠载情况。

不要使用扬声器直放麦克风声音，以免回授。测试不通过时先提供具体错误、设备格式和是否开启 probe；不通过自动重跑掩盖故障。安装插件不会放宽 AI 工具的实时执行限制。

## 分发限制

沿用 `third_party/rnnoise/README.integration.md` 中尚未解决的预训练权重许可边界：源代码与相关通知可审查，含模型的 DLL/包仅用于当前本机实验，暂不公开发布或放入仓库。包内保留 RNNoise 源码许可和模型来源/限制说明；它们不代表已取得权重再分发许可。
