# 固定依赖

`nlohmann/json` v3.12.0，MIT 许可证。使用官方单头文件，构建时不访问网络。

- 来源：https://github.com/nlohmann/json/tree/v3.12.0
- `nlohmann/json.hpp` SHA256：`aaf127c04cb31c406e5b04a63f1ae89369fccde6d8fa7cdda1ed4f32dfc5de63`
- `nlohmann/LICENSE.MIT` SHA256：`46a65cffd1ea955132d95a8dd921640714a8d6b537d2e4e482d31145ae95b603`

升级时更新文件、许可与摘要，并重跑配置解析、CLI 协议和核心测试。

## RNNoise 0.2

用于 `rnnoise_denoise` 离线语音降噪节点，固定官方 v0.2 源码及其指定的默认模型 `0b50c45`。模型作为本地依赖由脚本显式准备，Git忽略，不随源码分发；之后构建和运行不联网，不需要 Python 或 GPU。

- 上游源码 commit：`904a876dce1f9ab8860c0a5000ed151f9f6eef58`。
- 官方模型包 SHA256：`4ac81c5c0884ec4bd5907026aaae16209b7b76cd9d7f71af582094a2f98f4b43`。
- 默认模型 C 源码 SHA256：`522b6a64fded05bf85e58c06206eafe57ce7d94f3af58c725b17628b481d7890`。
- 代码的BSD-style许可、模型许可待澄清的限制、逐文件哈希、来源、x86-64/SSE2构建约束见 [RNNoise 集成说明](rnnoise/README.integration.md)、[COPYING](rnnoise/COPYING) 与 [SHA256SUMS](rnnoise/SHA256SUMS)。不能把代码许可自动视为独立权重的授权。
- 不包含训练数据/训练checkpoint，生成的默认模型源码约29.3 MB；源码大小不等于最终程序增加量。发布二进制时需附带这些第三方许可声明。

## miniaudio 0.11.23

用于 WASAPI 实时设备输入输出；选用 MIT No Attribution（MIT-0）许可，完整许可证包含上游的双许可文本。

- 来源：https://github.com/mackron/miniaudio/tree/0.11.23
- `miniaudio/miniaudio.h` SHA256：`7e4f3f13c8fe66df2080ac3dd12a89193e3c2463cb7f067c798abd7331cd8ee6`
- `miniaudio/LICENSE` SHA256：`457f1b500e0adf6bc059edddfa78a2f62012e7c3bb43476c20e0bd23b25ba0eb`
- `src/miniaudio_impl.c` 实例化实现；Windows统一裁剪到 WASAPI 设备API，未启用高级引擎/编解码。P10复用其线性插值＋四阶低通重采样DSP，不另引入库；非Windows的miniaudio目标只编译无设备/线程路径（整体工程当前仍受RNNoise x86-64集成限制）。
