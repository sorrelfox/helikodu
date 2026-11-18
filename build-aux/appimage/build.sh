#!/bin/sh
# SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Builds the Helikodu AppImage for the architecture of this machine, inside
# the Debian 13 container of Containerfile (Podman or Docker).
#
#   build-aux/appimage/build.sh [--source DIR|TARBALL] [--out DIR] [--cache DIR] [--offline]
#
#   --source   the sources to build: a checkout (default: this one) or a
#              source tarball made by `cargo xtask dist`
#   --out      where the AppImage goes, with the FFmpeg and vgmstream source
#              archives it was built from (default: target/appimage)
#   --cache    downloads, and the Rust toolchain and crates of local builds
#              (default: target/appimage-cache)
#   --offline  build the Rust crates offline (the vendored crates of a source
#              tarball); the release workflow uses it
#
# The pinned sources and tools of pins.sh are downloaded into the cache
# folder and checked against their SHA-256. The container runs with at most
# 12 GB of memory and CARGO_BUILD_JOBS (default 4) jobs. See docs/dev/appimage.md.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
. "$here/pins.sh"

source=$repo
out=$repo/target/appimage
cache=$repo/target/appimage-cache
offline=false
while [ $# -gt 0 ]; do
  case "$1" in
    --source) source=$2; shift 2 ;;
    --out) out=$2; shift 2 ;;
    --cache) cache=$2; shift 2 ;;
    --offline) offline=true; shift ;;
    -h | --help) sed -n '5,20p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "build.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done

arch=$(uname -m)
case "$arch" in
  x86_64 | aarch64) ;;
  *) echo "build.sh: no AppImage for $arch" >&2; exit 1 ;;
esac

engine=${HELIKODU_CONTAINER_ENGINE:-}
if [ -z "$engine" ]; then
  if command -v podman >/dev/null 2>&1; then
    engine=podman
  elif command -v docker >/dev/null 2>&1; then
    engine=docker
  else
    echo "build.sh: needs Podman or Docker" >&2
    exit 1
  fi
fi

mkdir -p "$out" "$cache/downloads"
out=$(cd "$out" && pwd)
cache=$(cd "$cache" && pwd)
source=$(cd "$(dirname "$source")" && pwd)/$(basename "$source")

# fetch NAME URL SHA256: downloads into the cache unless the file is there
# with the right checksum.
fetch() {
  file="$cache/downloads/$1"
  if [ -f "$file" ] && echo "$3  $file" | sha256sum --check --status; then
    return
  fi
  echo "Downloading $2"
  curl --proto '=https' --tlsv1.2 -fsSL -o "$file.part" "$2"
  if ! echo "$3  $file.part" | sha256sum --check --status; then
    echo "build.sh: $2 does not have the SHA-256 $3" >&2
    rm -f "$file.part"
    exit 1
  fi
  mv "$file.part" "$file"
}

pinned() {
  # The checksum of a per-architecture file: <NAME>_SHA256_<arch>.
  eval "echo \"\${$1_SHA256_$arch}\""
}

fetch "ffmpeg-$FFMPEG_VERSION.tar.xz" "$FFMPEG_URL" "$FFMPEG_SHA256"
fetch "vgmstream-$VGMSTREAM_VERSION.tar.gz" "$VGMSTREAM_URL" "$VGMSTREAM_SHA256"
fetch "linuxdeploy-$arch.AppImage" "$(echo "$LINUXDEPLOY_URL" | sed "s/@ARCH@/$arch/")" "$(pinned LINUXDEPLOY)"
fetch "appimagetool-$arch.AppImage" "$(echo "$APPIMAGETOOL_URL" | sed "s/@ARCH@/$arch/")" "$(pinned APPIMAGETOOL)"
fetch "runtime-$arch" "$(echo "$RUNTIME_URL" | sed "s/@ARCH@/$arch/")" "$(pinned RUNTIME)"

image=helikodu-appimage:debian13
"$engine" build -t "$image" -f "$here/Containerfile" "$here"

# The sources are mounted read-only; a tarball is unpacked inside.
if [ -d "$source" ]; then
  source_mount="$source:/src:ro"
  source_arg=/src
else
  source_mount="$(dirname "$source"):/srcdir:ro"
  source_arg="/srcdir/$(basename "$source")"
fi

# A checkout's commit time keeps the build reproducible; a tarball's files
# carry it already.
# An empty SOURCE_DATE_EPOCH is an error for clang, so it is only passed on
# with a value.
epoch=${SOURCE_DATE_EPOCH:-}
if [ -z "$epoch" ] && [ -d "$source" ]; then
  epoch=$(git -C "$source" log -1 --format=%ct 2>/dev/null || true)
fi

# label=disable: the mounts are used as they are, without relabelling the
# folders on hosts with SELinux.
"$engine" run --rm --memory 12g --memory-swap 12g --security-opt label=disable \
  -e CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" -e CARGO_TERM_COLOR \
  -e ARCH="$arch" -e OFFLINE="$offline" ${epoch:+-e SOURCE_DATE_EPOCH="$epoch"} \
  -e FFMPEG_VERSION="$FFMPEG_VERSION" -e VGMSTREAM_VERSION="$VGMSTREAM_VERSION" \
  -v "$here:/recipe:ro" \
  -v "$source_mount" \
  -v "$cache:/cache" \
  -v "$out:/out" \
  "$image" sh /recipe/build-in-container.sh "$source_arg"

# The source archives of FFmpeg and vgmstream go beside the image: a release
# publishes them with it, as the sources of the binaries in the image.
cp "$cache/downloads/ffmpeg-$FFMPEG_VERSION.tar.xz" "$cache/downloads/vgmstream-$VGMSTREAM_VERSION.tar.gz" "$out/"

ls -l "$out"
