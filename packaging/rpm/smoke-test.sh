#!/bin/sh
# SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Installs the built RPM in a clean Fedora container, runs the programs, and
# removes it again (run as root there):
#
#   podman run --rm -v "$PWD:/src:Z" registry.fedoraproject.org/fedora:45 \
#       sh /src/packaging/rpm/smoke-test.sh /src/target/rpm/helikodu-0.9.0-1.fc45.x86_64.rpm
#
# Plays to a WAV file only, never to a sound device.
set -eu

rpm=$(realpath "$1")

# Fedora's container image installs neither documentation (the manual pages)
# nor translations other than en_US; this throwaway container installs both,
# as a desktop system does.
echo '%_install_langs all' > /etc/rpm/macros.image-language-conf
dnf install -y --setopt=tsflags= "$rpm"
rpm -q helikodu
rpm -V helikodu

# Both programs start with the libraries the package depends on, and are
# the version the package is named for.
version=$(rpm -q --queryformat '%{VERSION}' helikodu | tr '~' -)
helikodu --version
helikodu --version | grep -Fx "helikodu $version"
helikodu-cli --version
test -x /usr/libexec/helikodu/helikodu-scan-worker
test -s /usr/share/helikodu/guide/index.html
test -s /usr/share/locale/de/LC_MESSAGES/helikodu.mo
test -e /usr/share/man/man1/helikodu-cli.1.gz

# A home of its own, and one second of 16-bit stereo silence at 44.1 kHz as
# the library.
work=$(mktemp -d)
export HOME="$work/home"
export XDG_DATA_HOME="$HOME/.local/share" XDG_CONFIG_HOME="$HOME/.config" XDG_CACHE_HOME="$HOME/.cache"
mkdir -p "$HOME" "$work/music"
{
    printf 'RIFF\064\261\002\000WAVEfmt \020\000\000\000\001\000\002\000'
    printf '\104\254\000\000\020\261\002\000\004\000\020\000data\020\261\002\000'
    head -c 176400 /dev/zero
} > "$work/music/silence.wav"

helikodu-cli scan "$work/music"
helikodu-cli search silence
helikodu-cli stats
helikodu-cli play "$work/music/silence.wav" --backend wav --wav-out "$work/out.wav" </dev/null
test -s "$work/out.wav"

dnf remove -y helikodu
if rpm -q helikodu; then exit 1; fi
test ! -e /usr/bin/helikodu
test ! -e /usr/libexec/helikodu
test ! -e /usr/share/helikodu
echo "smoke test passed"
