#!/usr/bin/env bash
set -euo pipefail
# All environment changes are confined to this process. No package installation.
export PATH="$AUDIO_BUILD_ROOT/tools/usr/bin:/mingw64/bin:/usr/bin:/bin:$AUDIO_CMP_DIRECTORY"
export LC_ALL=C
root="$AUDIO_BUILD_ROOT"
stage="$root/stage"
mkdir -p "$root/tools" "$root/src/lame" "$root/lame-build" "$root/ffmpeg-build" "$root/metadata"
for tool in awk sed grep expr dirname basename tr sort head tail cat cut printf od wc uname mktemp cp mv rm mkdir install ln touch cmp; do
    command -v "$tool" || { echo "Missing build utility: $tool" >&2; exit 1; }
done > "$root/metadata/tool-command-paths.txt"
if [[ ! -x "$root/tools/usr/bin/make.exe" ]]; then
    tar -xf "$AUDIO_CACHE_ROOT/$AUDIO_MAKE_ARCHIVE" -C "$root/tools"
fi
if [[ ! -f "$root/src/lame/configure" ]]; then
    tar -xf "$AUDIO_CACHE_ROOT/$AUDIO_LAME_ARCHIVE" -C "$root/src/lame"
fi
make_tool="$root/tools/usr/bin/make.exe"
gcc --version > "$root/metadata/gcc-identification.txt"
gcc -v >> "$root/metadata/gcc-identification.txt" 2>&1
"$make_tool" --version > "$root/metadata/make-identification.txt"
cd "$root/lame-build"
if [[ ! -f .audio-configured ]]; then
    MAKE="$make_tool" ../src/lame/configure --prefix="$stage" --host=x86_64-w64-mingw32 \
        --enable-shared --disable-static --disable-cpml --disable-frontend --disable-decoder \
        CFLAGS='-O2 -DNDEBUG' LDFLAGS='-static-libgcc' > configure.log 2>&1
    touch .audio-configured
fi
"$make_tool" -j"$AUDIO_JOBS" > build.log 2>&1
"$make_tool" install > install.log 2>&1
cp config.h "$root/metadata/lame-config.h"
cp config.log "$root/metadata/lame-config.log"
printf '%s\n' "--prefix=$stage" --host=x86_64-w64-mingw32 --enable-shared --disable-static \
    --disable-cpml --disable-frontend --disable-decoder 'CFLAGS=-O2 -DNDEBUG' 'LDFLAGS=-static-libgcc' \
    > "$root/metadata/lame-configure-flags.txt"
cd "$root/ffmpeg-build"
export PKG_CONFIG_PATH="$stage/lib/pkgconfig"
flags=(
    --prefix="$stage" --target-os=mingw32 --arch=x86_64 --cc=gcc
    --enable-shared --disable-static --disable-autodetect --disable-gpl --disable-nonfree
    --disable-network --disable-x86asm --disable-doc --disable-debug --disable-everything
    --disable-ffplay --enable-ffmpeg --enable-ffprobe --disable-avdevice --disable-swscale
    --enable-avcodec --enable-avformat --enable-avutil --enable-swresample --enable-avfilter
    --enable-libmp3lame --enable-protocol=file,pipe
    --enable-decoder="$(cat "$root/metadata/decoder-configure-ids.txt")" --enable-encoder="$AUDIO_ENCODERS"
    --enable-demuxer="$AUDIO_DEMUXERS" --enable-muxer="$AUDIO_MUXERS"
    --enable-parser=aac,aac_latm,ac3,flac,mpegaudio,opus,vorbis
    --enable-bsf=aac_adtstoasc,extract_extradata --enable-filter="$AUDIO_FILTERS"
    --extra-cflags="-O2 -I$stage/include" --extra-ldflags="-L$stage/lib -static-libgcc"
)
printf '%s\n' "${flags[@]}" > "$root/metadata/configure-flags.txt"
if [[ ! -f .audio-configured ]]; then
    "$root/src/FFmpeg-$AUDIO_FFMPEG_COMMIT/configure" "${flags[@]}" > configure.log 2>&1
    touch .audio-configured
fi
"$make_tool" -j"$AUDIO_JOBS" > build.log 2>&1
"$make_tool" install > install.log 2>&1
cp config.h config_components.h "$root/metadata/"
cp ffbuild/config.mak "$root/metadata/ffmpeg-config.mak"
export PATH="$stage/bin:$PATH"
ffmpeg -version > "$root/metadata/version.txt" 2>&1
ffmpeg -buildconf > "$root/metadata/buildconf.txt" 2>&1
ffmpeg -L > "$root/metadata/license.txt" 2>&1
ffmpeg -hide_banner -decoders > "$root/metadata/decoders.txt" 2>&1
ffmpeg -hide_banner -encoders > "$root/metadata/encoders.txt" 2>&1
ffmpeg -hide_banner -formats > "$root/metadata/formats.txt" 2>&1
ffmpeg -hide_banner -protocols > "$root/metadata/protocols.txt" 2>&1
ffmpeg -hide_banner -hwaccels > "$root/metadata/hwaccels.txt" 2>&1
for pe in "$stage"/bin/*.exe "$stage"/bin/*.dll; do
    printf '\nFILE: %s\n' "$(basename "$pe")"
    objdump -p "$pe" | grep 'DLL Name:'
done > "$root/metadata/pe-imports.txt"
