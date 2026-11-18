#!/bin/sh
# SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Builds the Arch Linux package from a release source tarball, checks it with
# namcap, installs it, tries the programs (no sound: the CLI plays into a WAV
# file) and removes it again. Runs as root in an archlinux:latest container;
# makepkg itself runs as an unprivileged user.
#
#   build-package.sh SOURCE-TARBALL OUT-DIR
#
# The package, helikodu-<version>-<pkgrel>-<arch>.pkg.tar.zst, lands in OUT-DIR.
set -eu

tarball=$(realpath "$1")
out=$(realpath -m "$2")
here=$(dirname "$(realpath "$0")")

name=$(basename "$tarball" .tar.xz)
srcver=${name#helikodu-}
case "$srcver" in
  "" | *[!0-9A-Za-z.-]*) echo "unexpected tarball name: $tarball" >&2; exit 1 ;;
esac

pacman -Syu --noconfirm --needed base-devel namcap sudo

# makepkg refuses to run as root; its --syncdeps installs the dependencies
# through sudo.
id builder >/dev/null 2>&1 || useradd --create-home builder
echo 'builder ALL=(ALL) NOPASSWD: /usr/bin/pacman' > /etc/sudoers.d/builder
chmod 0440 /etc/sudoers.d/builder

work=/tmp/helikodu-arch
rm -rf "$work"
mkdir -p "$work"
cp "$here/PKGBUILD" "$work/"
cp "$tarball" "$work/"
sum=$(sha256sum "$tarball" | cut -d' ' -f1)
sed -i -e "s/^_srcver=.*/_srcver=$srcver/" \
       -e "s/^sha256sums=.*/sha256sums=('$sum')/" "$work/PKGBUILD"
chown -R builder: "$work"

cd "$work"
sudo -u builder env CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" \
  makepkg --syncdeps --noconfirm --cleanbuild

pkgver=$(sudo -u builder makepkg --printsrcinfo | sed -n 's/^\tpkgver = //p')
pkgrel=$(sudo -u builder makepkg --printsrcinfo | sed -n 's/^\tpkgrel = //p')
pkg="helikodu-$pkgver-$pkgrel-$(uname -m).pkg.tar.zst"
test -s "$pkg"

# namcap reports; its warnings are listed, not fatal (it cannot know that the
# programs load libjack at run time, for example).
namcap PKGBUILD
namcap "$pkg"

# Install, try and remove the package. The archlinux image tells pacman not
# to extract manual pages and translations; let it, to check that they are in
# the package.
sed -i '/^NoExtract/d' /etc/pacman.conf
pacman -U --noconfirm "$pkg"
# Both programs start with the libraries of the package's dependencies and
# are the version the package is named for.
helikodu --version
helikodu --version | grep -Fx "helikodu $srcver"
helikodu-cli --version
if ldd /usr/bin/helikodu /usr/bin/helikodu-cli /usr/lib/helikodu/helikodu-scan-worker | grep 'not found'; then
  exit 1
fi
test -x /usr/lib/helikodu/helikodu-scan-worker
test ! -e /usr/libexec/helikodu
test -s /usr/share/helikodu/guide/index.html
test -s /usr/share/applications/io.github.sorrelfox.Helikodu.desktop
test -s /usr/share/metainfo/io.github.sorrelfox.Helikodu.metainfo.xml
test -s /usr/share/dbus-1/services/io.github.sorrelfox.Helikodu.service
test -s /usr/share/man/man1/helikodu-cli.1.gz
for lang in $(sed "s/#.*//" "$work/src/helikodu-$srcver/po/LINGUAS"); do
  test -s "/usr/share/locale/$lang/LC_MESSAGES/helikodu.mo"
done

sudo -u builder sh -euc '
  export HOME=$(mktemp -d) XDG_RUNTIME_DIR=$(mktemp -d)
  mkdir -p "$HOME/Music/Album"
  ffmpeg -loglevel error -f lavfi -i "sine=frequency=440:duration=2" \
    -metadata title=Tone -metadata artist=Test -metadata album=Album \
    "$HOME/Music/Album/01.flac"
  helikodu-cli scan "$HOME/Music"
  helikodu-cli search Tone
  helikodu-cli play "$HOME/Music/Album/01.flac" --backend wav --wav-out "$HOME/out.wav" </dev/null
  test -s "$HOME/out.wav"
  rm -rf "$HOME" "$XDG_RUNTIME_DIR"
'

pacman -R --noconfirm helikodu
if pacman -Qq helikodu >/dev/null 2>&1; then echo 'still installed' >&2; exit 1; fi
for path in /usr/bin/helikodu /usr/bin/helikodu-cli /usr/lib/helikodu /usr/share/helikodu; do
  if [ -e "$path" ]; then echo "left behind after removal: $path" >&2; exit 1; fi
done

mkdir -p "$out"
cp "$pkg" "$out/"
ls -l "$out"
