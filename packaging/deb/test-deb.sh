#!/bin/sh
# SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Installs a Helikodu .deb in a fresh container of its distribution, checks
# the programs and the installed files, scans and plays a generated file to
# the WAV and null backends (no sound), then removes the package and checks that
# nothing is left behind. Run as root:
#
#   packaging/deb/test-deb.sh PACKAGE.deb
set -eu

deb=$(realpath "$1")
export DEBIAN_FRONTEND=noninteractive

# The minimal Ubuntu container image leaves out manual pages, translations
# and documentation when packages are installed; a desktop system keeps them.
rm -f /etc/dpkg/dpkg.cfg.d/excludes

apt-get update
# apt resolves the dependencies from ${shlibs:Depends}.
apt-get install -y --no-install-recommends "$deb"

# Both programs start with the libraries the package depends on, and are
# the version the package is named for (the upstream part of the package
# version, with the tilde of a pre-release back as a hyphen).
version=$(dpkg-query --showformat '${Version}' --show helikodu)
version=$(echo "${version%-*}" | tr '~' -)
helikodu --version
helikodu --version | grep -Fx "helikodu $version"
helikodu-cli --version
test -x /usr/libexec/helikodu/helikodu-scan-worker
for file in \
    /usr/share/applications/io.github.sorrelfox.Helikodu.desktop \
    /usr/share/metainfo/io.github.sorrelfox.Helikodu.metainfo.xml \
    /usr/share/dbus-1/services/io.github.sorrelfox.Helikodu.service \
    /usr/share/icons/hicolor/scalable/apps/io.github.sorrelfox.Helikodu.svg \
    /usr/share/icons/hicolor/symbolic/apps/io.github.sorrelfox.Helikodu-symbolic.svg \
    /usr/share/man/man1/helikodu.1.gz \
    /usr/share/man/man1/helikodu-cli.1.gz \
    /usr/share/man/man1/helikodu-scan-worker.1.gz \
    /usr/share/helikodu/guide/index.html \
    /usr/share/doc/helikodu/copyright; do
    test -s "$file"
done
grep -qx "Exec=/usr/bin/helikodu --gapplication-service" \
    /usr/share/dbus-1/services/io.github.sorrelfox.Helikodu.service
languages=$(ls /usr/share/locale/*/LC_MESSAGES/helikodu.mo | wc -l)
echo "translations: $languages"
test "$languages" -ge 11

# One second of 16-bit stereo noise at 44.1 kHz, as a WAV file.
music=$(mktemp -d)
le32() { printf "\\$(printf %03o $(($1 & 255)))\\$(printf %03o $(($1 >> 8 & 255)))\\$(printf %03o $(($1 >> 16 & 255)))\\$(printf %03o $(($1 >> 24 & 255)))"; }
{
    printf RIFF; le32 176436; printf WAVEfmt
    printf ' \020\000\000\000\001\000\002\000'
    le32 44100; le32 176400
    printf '\004\000\020\000data'; le32 176400
    head -c 176400 /dev/urandom
} > "$music/noise.wav"

export HOME="$(mktemp -d)"
helikodu-cli scan "$music" --database "$HOME/library.db"
helikodu-cli search noise --database "$HOME/library.db"
helikodu-cli play "$music/noise.wav" --backend wav --wav-out "$HOME/out.wav"
test -s "$HOME/out.wav"
helikodu-cli play "$music/noise.wav" --backend null

apt-get purge -y helikodu
left=$(dpkg -L helikodu 2>/dev/null || true)
if [ -n "$left" ] || [ -e /usr/bin/helikodu ] || [ -e /usr/libexec/helikodu ] || [ -e /usr/share/helikodu ]; then
    echo "files left after removal" >&2
    exit 1
fi
echo "deb test passed"
