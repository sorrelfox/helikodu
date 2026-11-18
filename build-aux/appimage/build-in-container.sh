#!/bin/sh
# SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Builds the AppImage inside the container of Containerfile; started by
# build.sh, which mounts:
#
#   /recipe   this folder (read-only)
#   /src      the sources: a checkout, or the folder of a source tarball
#   /cache    downloads/ with the pinned files of pins.sh; the Rust toolchain
#             and crates of local builds are kept here as well
#   /out      where the AppImage goes
#
# Environment: ARCH, OFFLINE (true/false), CARGO_BUILD_JOBS, SOURCE_DATE_EPOCH,
# FFMPEG_VERSION, VGMSTREAM_VERSION.
#
# Steps: a minimal FFmpeg and libvgmstream are built into /opt/deps; Meson
# builds and installs Helikodu into the AppDir; linuxdeploy copies the shared
# libraries the programs need; appimagetool packs the image. Then the image
# is checked: no unexpected library from the system, the glibc version it
# needs, the programs start.
set -eu

source_arg=$1
downloads=/cache/downloads
jobs=${CARGO_BUILD_JOBS:-4}
deps=/opt/deps
work=/work
appdir=$work/AppDir
triplet=$(gcc -dumpmachine)
app_id=io.github.sorrelfox.Helikodu

case "$(uname -m)" in
  "$ARCH") ;;
  *) echo "built on $(uname -m), expected $ARCH" >&2; exit 1 ;;
esac

rm -rf "$work"
mkdir -p "$work" "$deps"

# --- The sources --------------------------------------------------------------

if [ -d "$source_arg" ]; then
  src=$source_arg
else
  mkdir -p "$work/source"
  tar -xJf "$source_arg" -C "$work/source"
  src=$(find "$work/source" -mindepth 1 -maxdepth 1 -type d)
fi
version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' "$src/Cargo.toml")
echo "Helikodu $version for $ARCH from $src"

# FFmpeg and vgmstream are found here by vgmstream's and Helikodu's builds.
export PKG_CONFIG_PATH="$deps/lib/pkgconfig"

# Local builds keep FFmpeg and vgmstream between runs, as long as their part
# of this script and the pinned versions stay the same; release builds always
# build them.
deps_key="$ARCH-$FFMPEG_VERSION-$VGMSTREAM_VERSION-$(sed -n '/^# --- FFmpeg/,/^# --- Helikodu/p' "$0" | sha256sum | cut -c1-16)"
deps_cache=/cache/deps/$deps_key
if [ "$OFFLINE" != true ] && [ -f "$deps_cache/lib/libvgmstream.so" ]; then
  cp -a "$deps_cache/." "$deps/"
  mkdir -p "$work/ffmpeg" "$work/vgmstream"
  tar -xJf "$downloads/ffmpeg-$FFMPEG_VERSION.tar.xz" -C "$work/ffmpeg" --strip-components=1
  tar -xzf "$downloads/vgmstream-$VGMSTREAM_VERSION.tar.gz" -C "$work/vgmstream" --strip-components=1
  deps_cached=true
else
  deps_cached=false
fi

if [ "$deps_cached" = false ]; then

# --- FFmpeg -------------------------------------------------------------------
#
# LGPL (no --enable-gpl, no --enable-nonfree), shared libraries, and only:
# - the demuxers of the bridge's format whitelist (FORMAT_WHITELIST in
#   crates/ffmpeg-bridge/src/io/lockdown.rs);
# - the audio decoders: the bridge allows every audio decoder of the build
#   (codec_whitelist), so they are all here, and no video, image or subtitle
#   decoder;
# - the audio parsers the demuxers use;
# - the encoders and muxers the converter offers (EncodeFormat in
#   crates/ffmpeg-bridge/src/encode.rs): Opus (libopus and FFmpeg's own),
#   Vorbis (libvorbis), MP3 (LAME), AAC, FLAC, ALAC and 16/24-bit PCM;
# - no protocols, no network, no devices, no filters, no scaling, no
#   hardware acceleration, nothing detected automatically.
# libswresample is built for vgmstream, which converts with it.

mkdir -p "$work/ffmpeg"
tar -xJf "$downloads/ffmpeg-$FFMPEG_VERSION.tar.xz" -C "$work/ffmpeg" --strip-components=1
cd "$work/ffmpeg"

demuxers=aac,ac3,adx,aea,aiff,amr,amrnb,amrwb,ape,asf,au,avi,avr,bit,caf,codec2,dsf,dts,dtshd,dv,eac3,flac,flv,gsm,hcom,iff,ilbc,ircam,lc3,loas,matroska,mlp,mmf,mov,mp3,mpc,mpc8,mpeg,mpegts,mxf,nistsphere,nut,oma,ogg,rm,shn,tak,truehd,tta,voc,vqf,w64,wav,wsaud,wsd,wv,xwma
# The demuxer list above must match the bridge's whitelist.
if [ -f "$src/crates/ffmpeg-bridge/src/io/lockdown.rs" ]; then
  whitelist=$(sed -n '/FORMAT_WHITELIST: &CStr = c"/,/";/p' "$src/crates/ffmpeg-bridge/src/io/lockdown.rs" \
    | sed 's/.*c"//; s/";.*//; s/\\$//' | tr -d ' \n')
  if [ "$whitelist" != "$demuxers" ]; then
    echo "The demuxers differ from FORMAT_WHITELIST in crates/ffmpeg-bridge:" >&2
    echo "  build:     $demuxers" >&2
    echo "  whitelist: $whitelist" >&2
    exit 1
  fi
fi
# Names FFmpeg does not have (some demuxers of the whitelist only exist with
# external libraries) are dropped instead of failing configure.
known_demuxers=$(./configure --list-demuxers | tr -s ' \t' '\n')
demuxers=$(echo "$demuxers" | tr ',' '\n' | grep -Fx "$known_demuxers" | paste -sd, -)

# Every audio decoder: the entries between "audio codecs" and "subtitles" in
# allcodecs.c (audio, PCM, DPCM and ADPCM), without the decoders there that
# are not audio.
decoders=$(sed -n '/\/\* audio codecs \*\//,/\/\* subtitles \*\//p' libavcodec/allcodecs.c \
  | sed -n 's/^extern const FFCodec ff_\(.*\)_decoder;$/\1/p' \
  | grep -vx 'hdr' | paste -sd, -)

parsers=aac,aac_latm,ac3,adx,ahx,amr,cook,dca,dolby_e,dvaudio,flac,ftr,g723_1,g729,gsm,misc4,mlp,mpegaudio,opus,sbc,sipr,tak,vorbis,xma

./configure \
  --prefix="$deps" \
  --enable-shared --disable-static --enable-pic \
  --disable-programs --disable-doc --disable-debug \
  --disable-autodetect --enable-pthreads --enable-zlib \
  --disable-network \
  --disable-everything \
  --disable-avdevice --disable-avfilter --disable-swscale \
  --enable-swresample \
  --enable-libopus --enable-libvorbis --enable-libmp3lame \
  --enable-demuxer="$demuxers" \
  --enable-decoder="$decoders" \
  --enable-parser="$parsers" \
  --enable-encoder=libopus,opus,libvorbis,libmp3lame,aac,flac,alac,pcm_s16le,pcm_s24le \
  --enable-muxer=opus,ogg,mp3,ipod,flac,wav \
  | tee "$work/ffmpeg-configure.txt"
grep -q '^License: LGPL' "$work/ffmpeg-configure.txt" || {
  echo "FFmpeg is not configured as LGPL" >&2
  exit 1
}
make -j"$jobs"
make install

# --- vgmstream ----------------------------------------------------------------
#
# The shared library only, with FFmpeg (the one above), mpg123, Vorbis and
# Speex from the system and its built-in G.722.1, G.719 and ATRAC9 decoders.
# CELT is off: vgmstream would download its sources.

mkdir -p "$work/vgmstream"
tar -xzf "$downloads/vgmstream-$VGMSTREAM_VERSION.tar.gz" -C "$work/vgmstream" --strip-components=1
cmake -S "$work/vgmstream" -B "$work/vgmstream-build" -G Ninja \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$deps" -DCMAKE_INSTALL_LIBDIR=lib \
  -DBUILD_SHARED_LIBS=ON -DBUILD_CLI=OFF -DBUILD_V123=OFF -DBUILD_AUDACIOUS=OFF \
  -DUSE_FFMPEG=ON -DUSE_MPEG=ON -DUSE_VORBIS=ON -DUSE_SPEEX=ON -DUSE_CELT=OFF \
  -DUSE_G7221=ON -DUSE_G719=ON -DUSE_ATRAC9=ON \
  | tee "$work/vgmstream-configure.txt"
# Every library must come from the system or /opt/deps, never downloaded.
if grep -Eiq 'download|git clone|fetch' "$work/vgmstream-configure.txt" \
  || [ -d "$work/vgmstream/dependencies" ]; then
  echo "vgmstream wanted to download a dependency" >&2
  exit 1
fi
cmake --build "$work/vgmstream-build" -j "$jobs" --target libvgmstream_shared
cmake --install "$work/vgmstream-build"
test -f "$deps/lib/libvgmstream.so"
sed -n 's/^#define FFMPEG_CONFIGURATION "\(.*\)"$/\1/p' "$work/ffmpeg/config.h" > "$deps/ffmpeg-configuration.txt"
if [ "$OFFLINE" != true ]; then
  rm -rf /cache/deps
  mkdir -p "$deps_cache"
  cp -a "$deps/." "$deps_cache/"
fi

fi

ffmpeg_configuration=$(cat "$deps/ffmpeg-configuration.txt")

# --- Helikodu -----------------------------------------------------------------

if [ -d /cache ] && [ "$OFFLINE" != true ]; then
  # Local builds keep the toolchain and the crates between runs.
  export RUSTUP_HOME=/cache/rustup
  mkdir -p /cache/cargo-registry
  ln -sfn /cache/cargo-registry "$CARGO_HOME/registry"
fi
cd "$src"
rustup toolchain install
export VGMSTREAM_LIB_DIR="$deps/lib"
# The linker resolves the libraries FFmpeg's own libraries depend on
# (libavcodec needs libswresample) from here; without it the aarch64 link
# fails, since /opt/deps is in no default search path.
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C link-arg=-Wl,-rpath-link,$deps/lib"
meson setup "$work/build" "$src" --prefix=/usr --buildtype=release \
  -Doffline="$OFFLINE" -Dbundled_sqlite=true -Dvgmstream=true
meson compile -C "$work/build"
meson install -C "$work/build" --destdir "$appdir"

for lang in $(sed 's/#.*//' "$src/po/LINGUAS"); do
  test -s "$appdir/usr/share/locale/$lang/LC_MESSAGES/helikodu.mo"
done
test -s "$appdir/usr/share/helikodu/guide/index.html"
test -x "$appdir/usr/libexec/helikodu/helikodu-scan-worker"

# --- The AppDir -----------------------------------------------------------------

# Nothing outside the image can start the programs over D-Bus: the service
# file names /usr/bin/helikodu, and a launcher that sees DBusActivatable=true
# would only try D-Bus. TryExec names a program that is not on the PATH.
rm -rf "$appdir/usr/share/dbus-1"
desktop="$appdir/usr/share/applications/$app_id.desktop"
sed -i '/^DBusActivatable=/d; /^TryExec=/d' "$desktop"
desktop-file-validate "$desktop"
appstreamcli validate --no-net "$appdir/usr/share/metainfo/$app_id.metainfo.xml"

# The manual pages are of no use inside the image.
rm -rf "$appdir/usr/share/man"

# GTK's settings schemas (the file chooser keeps its settings there).
mkdir -p "$appdir/usr/share/glib-2.0/schemas"
cp /usr/share/glib-2.0/schemas/org.gtk.gtk4.*.gschema.xml "$appdir/usr/share/glib-2.0/schemas/"
glib-compile-schemas "$appdir/usr/share/glib-2.0/schemas"

# Adwaita's icons, for desktops without them (GTK's own widgets use its
# symbolic icons). The desktop's icon theme comes first; its cursors are
# always the desktop's.
mkdir -p "$appdir/usr/share/icons/Adwaita"
cp -a /usr/share/icons/Adwaita/index.theme /usr/share/icons/Adwaita/symbolic \
  /usr/share/icons/Adwaita/scalable /usr/share/icons/Adwaita/16x16 \
  "$appdir/usr/share/icons/Adwaita/"
cp /usr/share/icons/hicolor/index.theme "$appdir/usr/share/icons/hicolor/"

# The SVG loader of gdk-pixbuf (the other formats GTK needs are built into
# gdk-pixbuf), beside the other libraries. The cache names it by file name
# only, which GModule finds through the run path of the libraries.
loader_src=/usr/lib/$triplet/gdk-pixbuf-2.0/2.10.0/loaders/libpixbufloader_svg.so
mkdir -p "$appdir/usr/lib/gdk-pixbuf-2.0"
cp "$loader_src" "$appdir/usr/lib/"
"/usr/lib/$triplet/gdk-pixbuf-2.0/gdk-pixbuf-query-loaders" "$loader_src" \
  | sed "s|\"$(dirname "$loader_src")/|\"|" > "$appdir/usr/lib/gdk-pixbuf-2.0/loaders.cache"
grep -q '^"libpixbufloader_svg.so"' "$appdir/usr/lib/gdk-pixbuf-2.0/loaders.cache"

# The dconf backend of GSettings, so that GTK's settings are kept.
mkdir -p "$appdir/usr/lib/gio/modules"
cp "/usr/lib/$triplet/gio/modules/libdconfsettings.so" "$appdir/usr/lib/gio/modules/"
# GTK's own module folder stays empty: no print backends or input method
# modules of the system's GTK are loaded (see AppRun).
mkdir -p "$appdir/usr/lib/gtk-4.0"

# linuxdeploy and appimagetool are AppImages themselves; without FUSE in the
# container they unpack themselves to run.
export APPIMAGE_EXTRACT_AND_RUN=1
tools=$work/tools
mkdir -p "$tools"
for tool in linuxdeploy appimagetool; do
  cp "$downloads/$tool-$ARCH.AppImage" "$tools/$tool"
  chmod +x "$tools/$tool"
done

# The libraries that stay the system's, besides linuxdeploy's own list (glibc,
# libstdc++, libgcc_s, ALSA, JACK, the X11, Wayland, OpenGL and font
# libraries):
# - PipeWire: the client library speaks the protocol of the PipeWire server
#   that runs, and loads the system's own SPA plugins and modules.
# - ALSA (on linuxdeploy's list): loads the system's ALSA plugins (pipewire,
#   pulse) and reads its configuration.
# - JACK is loaded at run time when the JACK output is chosen, never linked.
cd "$work"
LD_LIBRARY_PATH="$deps/lib" "$tools/linuxdeploy" --appdir "$appdir" \
  --executable "$appdir/usr/bin/helikodu" \
  --executable "$appdir/usr/bin/helikodu-cli" \
  --executable "$appdir/usr/libexec/helikodu/helikodu-scan-worker" \
  --library "$appdir/usr/lib/libpixbufloader_svg.so" \
  --desktop-file "$desktop" \
  --icon-file "$appdir/usr/share/icons/hicolor/scalable/apps/$app_id.svg" \
  --custom-apprun /recipe/AppRun \
  --exclude-library 'libpipewire-0.3.so*'

# Two libraries on linuxdeploy's list are bundled after all, because a bundled
# library needs their exact version: libharfbuzz-subset (GTK) needs the
# HarfBuzz it was built with, and libgcrypt (libsidplayfp) a libgpg-error at
# least as new as its own.
for lib in libharfbuzz.so.0 libgpg-error.so.0; do
  cp -L "/usr/lib/$triplet/$lib" "$appdir/usr/lib/$lib"
  patchelf --set-rpath '$ORIGIN' "$appdir/usr/lib/$lib"
done

# --- Licences -----------------------------------------------------------------

licences="$appdir/usr/share/licenses"
mkdir -p "$licences/helikodu" "$licences/ffmpeg" "$licences/vgmstream"
cp "$src/LICENSE" "$licences/helikodu/"
cp "$work/ffmpeg/COPYING.LGPLv2.1" "$work/ffmpeg/LICENSE.md" "$licences/ffmpeg/"
cp "$work/vgmstream/COPYING" "$licences/vgmstream/"
{
  echo "Helikodu $version, AppImage for $ARCH"
  echo
  echo "Helikodu is free software under the GNU General Public License,"
  echo "version 3 or later (helikodu/LICENSE). The image also carries the"
  echo "libraries below, each under its own licence."
  echo
  echo "FFmpeg $FFMPEG_VERSION, LGPL 2.1 or later, built from"
  echo "  https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz"
  echo "  with the configuration: $ffmpeg_configuration"
  echo
  echo "vgmstream $VGMSTREAM_VERSION (ISC-style licence, vgmstream/COPYING), built from"
  echo "  https://github.com/vgmstream/vgmstream/archive/refs/tags/$VGMSTREAM_VERSION.tar.gz"
  echo
  echo "The exact source archives of FFmpeg and vgmstream used for this image,"
  echo "ffmpeg-$FFMPEG_VERSION.tar.xz and vgmstream-$VGMSTREAM_VERSION.tar.gz, are published with"
  echo "each Helikodu release on https://github.com/sorrelfox/helikodu/releases"
  echo "and listed in its SHA256SUMS file."
  echo
  echo "From Debian 13 (copyright files in debian/<package>; the sources are at"
  echo "https://sources.debian.org/ and https://snapshot.debian.org/):"
} > "$licences/README.txt"
mkdir -p "$licences/debian"
for lib in "$appdir"/usr/lib/*.so*; do
  name=$(basename "$lib")
  case "$name" in
    libav*.so* | libswresample.so* | libvgmstream.so*) continue ;;
  esac
  owner=$(dpkg -S "/usr/lib/$triplet/$name" "/lib/$triplet/$name" 2>/dev/null | head -n1 | cut -d: -f1 || true)
  if [ -z "$owner" ]; then
    owner=$(dpkg -S "$name" 2>/dev/null | grep -v -- '-dev' | head -n1 | cut -d: -f1 || true)
  fi
  if [ -z "$owner" ]; then
    echo "No Debian package owns $name" >&2
    exit 1
  fi
  echo "$owner $name"
done | sort -u > "$work/bundled.txt"
for package in $(cut -d' ' -f1 "$work/bundled.txt" | sort -u); do
  mkdir -p "$licences/debian/$package"
  cp "/usr/share/doc/$package/copyright" "$licences/debian/$package/"
  files=$(awk -v p="$package" '$1 == p { print $2 }' "$work/bundled.txt" | paste -sd' ' -)
  echo "  $package $(dpkg-query -W -f='${Version}' "$package"): $files" >> "$licences/README.txt"
done

# --- The image ----------------------------------------------------------------

# No update information is embedded (no -u, no --guess): the image never
# looks for updates.
image_name="Helikodu-$version-$ARCH.AppImage"
ARCH=$ARCH VERSION=$version "$tools/appimagetool" --no-appstream \
  --runtime-file "$downloads/runtime-$ARCH" \
  "$appdir" "$work/$image_name"

# --- Checks ---------------------------------------------------------------------

# Every library the programs and libraries need is in the image, or one of
# those the system provides.
system_libs='ld-linux-x86-64.so.2 ld-linux-aarch64.so.1 libc.so.6 libm.so.6 libmvec.so.1 libdl.so.2 libpthread.so.0 librt.so.1 libresolv.so.2 libutil.so.1
libstdc++.so.6 libgcc_s.so.1 libz.so.1 libexpat.so.1 libuuid.so.1
libasound.so.2 libjack.so.0 libpipewire-0.3.so.0
libX11.so.6 libX11-xcb.so.1 libxcb.so.1 libICE.so.6 libSM.so.6 libwayland-client.so.0
libEGL.so.1 libGL.so.1 libGLX.so.0 libOpenGL.so.0 libGLdispatch.so.0 libdrm.so.2 libgbm.so.1
libfontconfig.so.1 libfreetype.so.6 libfribidi.so.0 libgmp.so.10 libcom_err.so.2'
find "$appdir" -type f \( -name '*.so*' -o -perm -u+x \) -exec sh -c 'file -b "$1" | grep -q "^ELF"' sh {} \; -print \
  > "$work/elf.txt"
missing=
for needed in $(xargs -a "$work/elf.txt" -d '\n' readelf -d 2>/dev/null | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p' | sort -u); do
  if [ -e "$appdir/usr/lib/$needed" ]; then
    continue
  fi
  if echo "$system_libs" | tr ' ' '\n' | grep -qx "$needed"; then
    echo "from the system: $needed"
    continue
  fi
  missing="$missing $needed"
done
if [ -n "$missing" ]; then
  echo "Libraries neither in the image nor expected from the system:$missing" >&2
  exit 1
fi

# The newest glibc symbol version any file needs.
glibc=$(xargs -a "$work/elf.txt" -d '\n' objdump -T 2>/dev/null | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -n1)
echo "Needs $glibc or newer"

# The programs start from the image (unpacked, as there is no FUSE here).
"$work/$image_name" --appimage-extract-and-run --version
"$work/$image_name" --appimage-extract-and-run --cli --version
"$work/$image_name" --appimage-extract-and-run --cli --help > /dev/null
# No update information, so nothing for AppImageUpdate to fetch.
if [ -n "$("$work/$image_name" --appimage-updateinformation 2>/dev/null)" ]; then
  echo "The image has update information" >&2
  exit 1
fi

cp "$work/$image_name" /out/
cp "$licences/README.txt" "/out/Helikodu-$version-$ARCH.AppImage.contents.txt"
echo "glibc: $glibc" >> "/out/Helikodu-$version-$ARCH.AppImage.contents.txt"
ls -l /out
