# RNNoise dependency integration

This directory contains the inference source subset of Xiph RNNoise **v0.2**, tag commit
`904a876dce1f9ab8860c0a5000ed151f9f6eef58`. The official default model **0b50c45** is
a separately prepared, Git-ignored local dependency, not redistributed in this repository.

- Source: https://github.com/xiph/rnnoise/tree/904a876dce1f9ab8860c0a5000ed151f9f6eef58
- Canonical upstream: https://gitlab.xiph.org/xiph/rnnoise
- Model: https://media.xiph.org/rnnoise/models/rnnoise_data-0b50c45.tar.gz
- Model archive SHA256: `4ac81c5c0884ec4bd5907026aaae16209b7b76cd9d7f71af582094a2f98f4b43`
- The pinned upstream `model_version` and `download_model.sh` identify this exact model.
- Source license: retain `COPYING` and notices in each source file (upstream BSD-style).
  The separately downloaded pretrained weights do not contain an explicit license.
  Upstream clarification is tracked at https://github.com/xiph/rnnoise/issues/284 .
  Do not infer permission to redistribute weights or binaries containing them from the
  source license. Resolve this before publishing such artifacts.

## Prepare for a local experiment

From the project root, explicitly acknowledge the outstanding model-license question:

```powershell
.\scripts\prepare-rnnoise-model.ps1 -AcknowledgeUnclearModelLicense
```

This downloads the fixed official archive only when needed, verifies archive and extracted
file SHA256 hashes, and refuses to overwrite unexpected existing files. It does not grant
redistribution rights. Without the switch, it only succeeds if both verified files are
already present (no network). Normal CMake configure/build never downloads a model; it
fails with preparation instructions on missing files or rejects changed hashes. After
preparation the build is offline. The local files remain ignored by Git.

The source files were exported from the pinned Git tree with automatic newline conversion
disabled; the model's `src/rnnoise_data.c` and `.h` were extracted unchanged from the archive.
`SHA256SUMS` records all 31 vendored files and the two local model files. No training checkpoint, dataset, trainer,
external-model loader UI, runtime download or Python dependency is included. The two
generated model files are approximately 29.3 MB of source text, not executable size.

Project-authored files: this document, `CMakeLists.txt`, `.gitattributes`, `SHA256SUMS`.
Upstream C/header files are unmodified. `.gitattributes` preserves imported bytes and hides
the generated weights from normal text diffs; verify weights by the recorded digest.

The CMake integration currently targets **x86-64**, using the upstream baseline SSE2 path
without requiring AVX2 or runtime CPU dispatch. MSVC C11 uses the upstream compatibility
macros `OPUS_X86_MAY_HAVE_SSE` and `OPUS_X86_MAY_HAVE_SSE2`. We deliberately do not run
upstream autotools/download scripts during configure or build. Other architectures need
a separate integration (the supplied scalar/NEON paths reference headers absent from v0.2).

The project wrapper assumes 480-sample frames, 48 kHz mono, PCM16-scale floats at the
library boundary, and 960 samples of delay for this version. A version/model upgrade must
revalidate delay/flush, model initialization, source hashes, licenses and tests together.
Each execution owns its state. Default-model state uses public preallocated initialization
and `free`, avoiding v0.2 `rnnoise_create`'s unchecked allocation.

Binary redistribution must carry the applicable copyright/license notices and resolve
the weights-license question above. Distributable installers are not enabled.
