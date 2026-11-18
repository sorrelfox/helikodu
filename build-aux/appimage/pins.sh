# SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The sources and tools the AppImage is built from, each pinned by version and
# SHA-256. build.sh downloads them into its cache folder and refuses a file
# whose checksum differs. Sourced by build.sh (POSIX sh).

# FFmpeg, built with only the parts Helikodu uses (see build-in-container.sh).
FFMPEG_VERSION=9.0.2
FFMPEG_URL="https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz"
FFMPEG_SHA256=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e

# vgmstream, for game audio streams (libvgmstream, public API 1.1).
VGMSTREAM_VERSION=r2168
VGMSTREAM_URL="https://github.com/vgmstream/vgmstream/archive/refs/tags/$VGMSTREAM_VERSION.tar.gz"
VGMSTREAM_SHA256=6390ac616c4a248616f1b9bf6352fe949fcf2350a84844ba9416feac94f02225

# linuxdeploy copies the shared libraries the programs need into the AppDir.
LINUXDEPLOY_VERSION=1-alpha-20251107-1
LINUXDEPLOY_URL="https://github.com/linuxdeploy/linuxdeploy/releases/download/$LINUXDEPLOY_VERSION/linuxdeploy-@ARCH@.AppImage"
LINUXDEPLOY_SHA256_x86_64=c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d
LINUXDEPLOY_SHA256_aarch64=620095110d693282b8ebeb244a95b5e911cf8f65f76c88b4b47d16ae6346fcff

# appimagetool packs the AppDir into the image.
APPIMAGETOOL_VERSION=1.9.1
APPIMAGETOOL_URL="https://github.com/AppImage/appimagetool/releases/download/$APPIMAGETOOL_VERSION/appimagetool-@ARCH@.AppImage"
APPIMAGETOOL_SHA256_x86_64=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
APPIMAGETOOL_SHA256_aarch64=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158

# The AppImage runtime (type 2, statically linked: needs no libfuse on the
# user's system). Given to appimagetool so that it downloads nothing itself.
RUNTIME_VERSION=20251108
RUNTIME_URL="https://github.com/AppImage/type2-runtime/releases/download/$RUNTIME_VERSION/runtime-@ARCH@"
RUNTIME_SHA256_x86_64=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d
RUNTIME_SHA256_aarch64=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444
