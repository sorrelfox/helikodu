#!/bin/sh
# SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Runs the scripted measurements of the UI spike in one environment and
# writes the results to results/<environment>.md.
#
#   ./measure.sh desktop      the current Wayland session (windows appear briefly)
#   ./measure.sh mutter       headless mutter on a private D-Bus session
#   ./measure.sh gnome-shell  headless GNOME Shell on a private D-Bus session
#   ./measure.sh kwin         headless KWin (virtual backend) on a private D-Bus session
#   ./measure.sh follow       headless mutter with a stand-in Settings portal that
#                             flips light/dark and the accent every 1.5 s
#
# Every run exits by itself. QUICK=0.2 shortens all phases for a smoke test.

set -eu
cd "$(dirname "$0")"
ENVIRONMENT=${1:-mutter}
QUICK=${QUICK:-1.0}
BIN=$PWD/target/release/ui-lists-spike

# Starts a headless compositor, runs the program against it, stops it.
headless() {
    comp=$1
    shift
    sock=hk-spike-$comp-$$
    case $comp in
    mutter) mutter --headless --no-x11 --virtual-monitor 1920x1080@60 --wayland-display="$sock" >/dev/null 2>&1 & ;;
    gnome-shell) gnome-shell --headless --no-x11 --virtual-monitor 1920x1080@60 --wayland-display="$sock" >/dev/null 2>&1 & ;;
    kwin) kwin_wayland --virtual --no-lockscreen --width 1920 --height 1080 --socket "$sock" >/dev/null 2>&1 & ;;
    esac
    cpid=$!
    i=0
    while [ ! -S "$XDG_RUNTIME_DIR/$sock" ] && [ $i -lt 100 ]; do
        sleep 0.1
        i=$((i + 1))
    done
    sleep 1
    WAYLAND_DISPLAY=$sock GDK_BACKEND=wayland GTK_A11Y=none "$@" || true
    kill $cpid 2>/dev/null || true
    wait $cpid 2>/dev/null || true
}

# Inside a private D-Bus session: ./measure.sh __inner <compositor> <program args...>
if [ "$ENVIRONMENT" = "__inner" ]; then
    comp=$2
    shift 2
    if [ "$comp" = "gnome-shell" ]; then
        export XDG_CURRENT_DESKTOP=GNOME
    fi
    headless "$comp" "$BIN" "$@"
    exit 0
fi

cargo build --release --quiet
mkdir -p results
OUT=$PWD/results/$ENVIRONMENT.md
: >"$OUT"

run() {
    label=$1
    shift
    echo "== $ENVIRONMENT: $label" >&2
    case $ENVIRONMENT in
    desktop)
        "$BIN" --label "$label" --out "$OUT" --time-scale "$QUICK" "$@" >/dev/null 2>&1 || true
        ;;
    *)
        dbus-run-session -- "$0" __inner "$ENVIRONMENT" --out "$OUT" --label "$label" --time-scale "$QUICK" "$@" >/dev/null 2>&1 || true
        ;;
    esac
}

if [ "$ENVIRONMENT" = "follow" ]; then
    for layout in "appmenu:close" "close,minimize,maximize:"; do
        echo "== follow: $layout" >&2
        dbus-run-session -- sh -c "\"$BIN\" fake-portal 1500 '$layout' & sleep 0.5; \"$0\" __inner mutter --out \"$OUT\" --view table --follow-only --label 'stand-in portal, button layout $layout'" >/dev/null 2>&1 || true
    done
    echo "results in $OUT" >&2
    exit 0
fi

run "200k tracks, 8 columns" --view table --rows 200000 --columns 8
run "200k tracks, 12 columns" --view table --rows 200000 --columns 12
run "200k tracks, 8 columns, no stylesheet" --view table --rows 200000 --columns 8 --no-css
run "100 tracks (memory baseline)" --view table --rows 100 --columns 8
run "20k albums, max-columns 12" --view grid --albums 20000 --max-columns 12
run "20k albums, max-columns 20" --view grid --albums 20000 --max-columns 20
echo "results in $OUT" >&2
