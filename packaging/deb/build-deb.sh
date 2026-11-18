#!/bin/sh
# SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Builds the Helikodu .deb from the release source tarball, inside a container
# of the distribution it is for (debian:13 or ubuntu:26.04), as root:
#
#   packaging/deb/build-deb.sh SOURCE_TARBALL OUT_DIR
#
# The debian/ folder is taken from packaging/deb/debian in the tarball. The
# package version is <version>-<revision>~<distribution><release>, such as
# 0.9.0-1~debian13, so that a later package of the distribution itself
# (0.9.0-1) replaces it; a pre-release suffix gets a tilde as well
# (0.9.0~rc1-1~debian13 sorts before 0.9.0-1~debian13). The file in OUT_DIR
# is named with dots in place of the tildes
# (helikodu_0.9.0-1.debian13_amd64.deb), because GitHub renames release files
# with a tilde in their name. OUT_DIR also gets the lintian report.
#
# Environment: DEB_REVISION (default 1), SOURCE_DATE_EPOCH (default: the
# date of the files in the tarball), CARGO_BUILD_JOBS (default 4).
set -eu

if [ $# -ne 2 ]; then
    echo "usage: $0 SOURCE_TARBALL OUT_DIR" >&2
    exit 2
fi
tarball=$(realpath "$1")
mkdir -p "$2"
out=$(realpath "$2")
revision=${DEB_REVISION:-1}
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
export DEBIAN_FRONTEND=noninteractive

name=$(basename "$tarball" .tar.xz)
version=${name#helikodu-}
. /etc/os-release
distro="$ID$VERSION_ID"
case "$ID" in
    debian | ubuntu) ;;
    *) echo "not a Debian or Ubuntu system: $ID" >&2; exit 1 ;;
esac
upstream=$(echo "$version" | tr - '~')
debversion="$upstream-$revision~$distro"

apt-get update
apt-get install -y --no-install-recommends ca-certificates curl xz-utils dpkg-dev lintian

work=$(mktemp -d)
tar -xJf "$tarball" -C "$work"
src="$work/$name"
cp -a "$src/packaging/deb/debian" "$src/debian"

# The changelog gives the version and, through its date, SOURCE_DATE_EPOCH.
if [ -z "${SOURCE_DATE_EPOCH:-}" ]; then
    SOURCE_DATE_EPOCH=$(stat -c %Y "$src/Cargo.toml")
fi
export SOURCE_DATE_EPOCH
cat > "$src/debian/changelog" <<EOF
helikodu ($debversion) $VERSION_CODENAME; urgency=medium

  * Helikodu $version for $NAME $VERSION_ID, built from the release source
    tarball. The changes are listed in /usr/share/doc/helikodu/changelog.gz.

 -- sorrelfox <348316453+sorrelfox@users.noreply.github.com>  $(date -u -R -d "@$SOURCE_DATE_EPOCH")
EOF

apt-get build-dep -y "$src"

# The Rust toolchain pinned in rust-toolchain.toml comes from rustup: the
# distributions' rustc is older than the minimum supported version.
export RUSTUP_HOME="${RUSTUP_HOME:-/usr/local/rustup}"
export CARGO_HOME="${CARGO_HOME:-/usr/local/cargo}"
export PATH="$CARGO_HOME/bin:$PATH"
if ! command -v rustup >/dev/null; then
    curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --no-modify-path --profile minimal --default-toolchain none
fi
(cd "$src" && rustup toolchain install)

(cd "$src" && dpkg-buildpackage -b -us -uc)

debarch=$(dpkg --print-architecture)
deb="$work/helikodu_${debversion}_${debarch}.deb"
asset=$(echo "helikodu_${debversion}_${debarch}.deb" | tr '~' .)

# Errors fail the build; warnings are shown in the report.
lintian --info --display-info --fail-on error "$deb" | tee "$out/$asset.lintian.txt"

cp "$deb" "$out/$asset"
dpkg-deb --info "$out/$asset"
dpkg-deb --contents "$out/$asset"
ls -l "$out"
