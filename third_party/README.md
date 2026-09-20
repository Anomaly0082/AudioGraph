# 固定依赖

`nlohmann/json` v3.12.0，MIT 许可证。使用官方单头文件，构建时不访问网络。

- 来源：https://github.com/nlohmann/json/tree/v3.12.0
- `nlohmann/json.hpp` SHA256：`aaf127c04cb31c406e5b04a63f1ae89369fccde6d8fa7cdda1ed4f32dfc5de63`
- `nlohmann/LICENSE.MIT` SHA256：`46a65cffd1ea955132d95a8dd921640714a8d6b537d2e4e482d31145ae95b603`

升级时更新文件、许可与摘要，并重跑配置解析、CLI 协议和核心测试。

## miniaudio 0.11.23

用于 WASAPI 实时设备输入输出；选用 MIT No Attribution（MIT-0）许可，完整许可证包含上游的双许可文本。

- 来源：https://github.com/mackron/miniaudio/tree/0.11.23
- `miniaudio/miniaudio.h` SHA256：`7e4f3f13c8fe66df2080ac3dd12a89193e3c2463cb7f067c798abd7331cd8ee6`
- `miniaudio/LICENSE` SHA256：`457f1b500e0adf6bc059edddfa78a2f62012e7c3bb43476c20e0bd23b25ba0eb`
- `src/miniaudio_impl.c` 实例化实现；CMake 统一裁剪到 WASAPI 设备API，未启用高级引擎/编解码。
