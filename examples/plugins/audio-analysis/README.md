# Audio analysis plugin

Independent Windows x64/MSVC C++20 plugin, using only the installed public `AudioGraph::PluginSDK` target. All configurations use the static `/MT` runtime. It has no model, network, filesystem, or audio-device operations and never modifies its borrowed input.

Build independently after installing SDK 0.1.0:

```powershell
cmake -S examples/plugins/audio-analysis -B build/audio-analysis -A x64 -DCMAKE_PREFIX_PATH=<installed-sdk-prefix>
cmake --build build/audio-analysis --config Release
```

The target `ag_audio_analysis_plugin` creates `org.audiograph.analysis.plugin.dll`. Its post-build package is `build/audio-analysis/package/Release/`: schema-2 `manifest.json`, the DLL, and `nodes.json`. The manifest hashes both payload files with SHA-256. No helper DLL or resource directory is needed. ABI 0.1 remains experimental.

The unmangled `ag_plugin_get_api` entry and every callback use SDK `AG_CALL` (`__cdecl` on Windows). Request ABI 0.1 exactly, then node `org.audiograph.analysis.audio_metrics_v1` with capability `ag.whole_sync/1`, version 1. Create accepts either no parameters (default -60 dB) or the single normalized Number parameter `silence_threshold_db`, finite in [-120, 0]. Supply a valid environment even though its paths are unused. Each successfully created instance allows one run attempt and must then be destroyed by this plugin.

Run takes exactly one required Audio input `audio` and emits four required Number outputs in order: `rms`, `rms_dbfs`, `clipping_ratio`, `silence_ratio`. Input must have at least one frame, a positive sample rate/channel count, valid interleaved float32 storage, and only finite samples. Finite values outside [-1, 1] are accepted without clamping. Input budget must cover all sample bytes and one port; output budget must cover 32 bytes and four ports.

RMS is `sqrt(sum(double(x)*x)/sample_count)` over all channels. dBFS is `max(-120, 20*log10(rms))`, or -120 for zero RMS; positive dBFS is permitted. Clipping ratio counts values strictly outside the PCM16 range `[-1, 32767/32768]`, matching core WAV output, and divides by total sample count.

Silence ratio measures low-level frame coverage, not speech detection/VAD. Fixed nominal 20 ms windows have `max(1, (uint64(sample_rate)+25)/50)` frames, rounding to the nearest whole frame. A window is low-level when its RMS across all channels is at most `10^(silence_threshold_db/20)`. Low-level windows contribute their actual frame counts, including the final short window; the total is divided by input frame count. Empty inputs are rejected because these ratios are undefined.

Cancellation/deadline queries occur at least every 4096 samples and before/after each emission. Sinks copy borrowed outputs immediately; if any sink fails or cancellation/deadline arrives, run fails and the host must discard the entire staged result, including previously emitted metrics. Errors use fixed bounded text; callback exceptions are contained at ABI entry points.
