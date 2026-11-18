#!/bin/sh
# SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Runs a command against a headless Wayland compositor on a private D-Bus
# session, so GTK tests and screenshots need no desktop and touch none:
#
#   crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test widgets
#   crates/ui-gtk/tests/headless.sh cargo run -p helikodu-ui-gtk --example screenshot
#
# Uses mutter (`mutter --headless`), or KWin (`kwin_wayland --virtual`) when
# mutter is missing.

set -eu
if [ "${1:-}" = "__inner" ]; then
    shift
    sock=helikodu-headless-$$
    if command -v mutter >/dev/null 2>&1; then
        mutter --headless --no-x11 --virtual-monitor 1920x1080@60 --wayland-display="$sock" >/dev/null 2>&1 &
    elif command -v kwin_wayland >/dev/null 2>&1; then
        kwin_wayland --virtual --no-lockscreen --width 1920 --height 1080 --socket "$sock" >/dev/null 2>&1 &
    else
        echo "headless.sh: neither mutter nor kwin_wayland is installed" >&2
        exit 1
    fi
    compositor=$!
    i=0
    while [ ! -S "$XDG_RUNTIME_DIR/$sock" ] && [ $i -lt 100 ]; do
        sleep 0.1
        i=$((i + 1))
    done
    status=0
    env -u DISPLAY WAYLAND_DISPLAY="$sock" GDK_BACKEND=wayland GTK_A11Y=none \
        HELIKODU_GTK_TESTS=1 "$@" || status=$?
    kill "$compositor" 2>/dev/null || true
    wait "$compositor" 2>/dev/null || true
    exit $status
fi
exec dbus-run-session -- "$0" __inner "$@"
