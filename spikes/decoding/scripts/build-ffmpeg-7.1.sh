#!/bin/sh
# SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Builds a minimal shared FFmpeg 7.1 into target/ffmpeg-7.1/prefix without root,
# so the spike can be built and run against the oldest supported FFmpeg.
#
# Usage: scripts/build-ffmpeg-7.1.sh
# Then:  PKG_CONFIG_PATH=target/ffmpeg-7.1/prefix/lib/pkgconfig \
#        LD_LIBRARY_PATH=target/ffmpeg-7.1/prefix/lib \
#        cargo run --release --target-dir target/ff71 -- all
set -eu

VERSION="${FFMPEG_VERSION:-7.1.5}"
HERE="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$HERE/target/ffmpeg-7.1"
PREFIX="$WORK/prefix"
mkdir -p "$WORK"
cd "$WORK"

if [ ! -f "ffmpeg-$VERSION.tar.xz" ]; then
    curl -fsSLO "https://ffmpeg.org/releases/ffmpeg-$VERSION.tar.xz"
fi
sha256sum "ffmpeg-$VERSION.tar.xz"
rm -rf "ffmpeg-$VERSION"
tar xf "ffmpeg-$VERSION.tar.xz"
cd "ffmpeg-$VERSION"

# Only what the spike decodes, plus the demuxers and protocols the lock-down
# tests attack (hls, concat, http) so that the defences are tested for real.
# The video decoders are built on purpose: the spike proves they are never opened.
./configure \
    --prefix="$PREFIX" \
    --enable-shared --disable-static \
    --disable-programs --disable-doc --disable-x86asm \
    --disable-everything \
    --disable-avdevice --disable-avfilter --disable-swscale --disable-swresample \
    --enable-decoder=mp3,mp3float,aac,aac_fixed,opus,flac,vorbis,wavpack,pcm_s16le,pcm_s24le,mpeg4,vp8,vp9,png,mjpeg \
    --enable-demuxer=mp3,aac,mov,ogg,matroska,flac,wv,wav,hls,concat,mpegts \
    --enable-parser=mpegaudio,aac,opus,flac,vorbis,mpeg4video,vp8,vp9,png,mjpeg \
    --enable-protocol=file,http,tcp \
    --enable-bsf=aac_adtstoasc

make -j"$(nproc)"
make install
echo "FFmpeg $VERSION installed to $PREFIX"
