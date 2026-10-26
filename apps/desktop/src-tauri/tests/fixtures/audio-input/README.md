# Audio decoder regression inputs

These fixed, synthetic 0.1 second mono 48 kHz inputs contain a constant PCM sample of 4096. They were generated from the included PCM16 WAV using FFmpeg n8.1.3-23-g5c4f5744cd-20261022 and the encoder named by each format (libmp3lame, aac, alac, flac, libvorbis, libopus or pcm_*). No third-party media is included.

The fixtures are checked in so decoder tests never require optional fixture encoders in the shipped audio runtime. The application output encoders remain independently tested. All fixture media is dedicated to the public domain under CC0-1.0.
