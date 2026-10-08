# Experimental realtime RNNoise plugin

Standalone Windows x64 plugin using the installed public AudioGraph Plugin SDK
(`ag.realtime_audio/1`), without linking `audio_core`. Plugin ID is
`org.audiograph.rnnoise`, node type is `org.audiograph.rnnoise.realtime_v1`,
version is `0.1.0`. There are no parameters. This is for local experiments only:
the separately prepared default model has unresolved redistribution licensing.
Do not publish weights or binaries containing them. See NOTICE and the pinned
dependency's `README.integration.md`. Configure/build never downloads a model.

From the repository root, with an already installed SDK and verified local model:

```powershell
cmake -S examples/plugins/rnnoise-realtime -B build/rnnoise-realtime -A x64 -DCMAKE_PREFIX_PATH=C:/path/to/sdk-install -DAG_RNNOISE_SOURCE_DIR=C:/AAAProject/AudioProcess/third_party/rnnoise
cmake --build build/rnnoise-realtime --config Release --target ag_rnnoise_realtime_plugin
```

The complete package is `build/rnnoise-realtime/package/Release/`: plugin DLL,
`nodes.json`, `manifest.json`, `resources/COPYING`, and `resources/NOTICE`. License notices are
manifest resources with SHA256 hashes. The SDK and static RNNoise use MSVC /MT
in Release (/MTd in Debug); the DLL has no dynamic RNNoise dependency.

## Streaming contract

Prepare accepts only 48,000 Hz mono float32; `maximum_frames` must be 1..65536.
Each block has 1..maximum_frames samples and is processed in place with exactly
the same length. Each instance owns independent preallocated default-model state,
480-sample input/output arrays, and a cursor. Prepare allocates checked state,
initializes it, touches part of the inference path with a silence frame, then reinitializes it so the initial
algorithm state and starting zero output match an unwarmed reference. The pinned
default-model inference path does not allocate lazily. The callback does no
allocation, file access, logging, or locking, and ABI callbacks catch exceptions.

Before advancing state or modifying a block, the entire input is checked for
finiteness, then for the normalized range [-1,1]. Non-finite input returns
`AG_RT_NON_FINITE_INPUT`; finite out-of-range input returns `AG_RT_NODE_FAILED`.
Input is never silently clamped. Samples are multiplied by 32768 at the RNNoise
boundary and divided by 32768 on output. Output finiteness is checked; output is
not clamped. Any processing error ends the session; use a fresh instance to retry.

For every sample the adapter first emits `output[position]` (initially zero),
stores the incoming sample in `input[position]`, then advances the cursor. At 480
samples RNNoise processes that frame into the next output buffer and resets the
cursor. Block boundaries therefore do not alter the algorithm frame sequence.
The adapter adds 480 samples to the pinned v0.2 algorithm's 960-sample delay:
total latency is **1440 samples / 30 ms**, excluding device/host buffering.

Stop/destroy does **not flush**: any partial input frame, queued output, and
algorithm-delayed tail are discarded. Output length always equals submitted input
length; the delayed tail is not automatically appended. A host wishing to drain
must explicitly submit silence before stopping and account for the added output.
A new session must create a fresh instance; prepare is accepted once per instance.
