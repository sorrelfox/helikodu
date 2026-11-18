#!/bin/sh
# SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Builds every fuzz target with the flags `cargo fuzz` uses (libFuzzer, coverage
# instrumentation, AddressSanitizer) and runs a campaign: each target for a fixed
# time, several at once. Needs only a nightly toolchain, not cargo-fuzz itself.
#
#   fuzz/campaign.sh [SECONDS_PER_TARGET] [PARALLEL_JOBS] [TARGET...]
#
# Defaults: 600 seconds, 4 jobs, every target. Seeds are taken from
# fuzz/seeds/<target>/ when that folder exists; corpora, logs and crashes go to
# fuzz/runs/<target>/ (corpus/, log.txt, artifacts/), which git ignores. The
# summary lists each target with its exit status and the number of crash files.

set -eu

here=$(cd "$(dirname "$0")" && pwd)
target_dir=${CARGO_TARGET_DIR:-$here/target}
triple=$(rustc +nightly -vV | sed -n 's/^host: //p')
bin="$target_dir/$triple/release"

# One target: `campaign.sh --one TARGET SECONDS`.
if [ "${1:-}" = "--one" ]; then
    t=$2
    seconds=$3
    work="$here/runs/$t"
    mkdir -p "$work/corpus" "$work/artifacts" "$work/tmp"
    if [ -d "$here/seeds/$t" ]; then
        cp -n "$here/seeds/$t"/* "$work/corpus/" 2>/dev/null || true
    fi
    case $t in
        *_open | tags_read | ffmpeg_encode | cover_decode) max_len=65536 ;;
        *) max_len=8192 ;;
    esac
    status=0
    (cd "$work" && TMPDIR="$work/tmp" ASAN_OPTIONS=allocator_may_return_null=1 \
        LSAN_OPTIONS=suppressions="$here/lsan.supp" \
        "$bin/$t" -max_total_time="$seconds" -rss_limit_mb=2560 -malloc_limit_mb=2048 \
        -timeout=20 -max_len="$max_len" -print_final_stats=1 \
        -artifact_prefix="$work/artifacts/" "$work/corpus" >"$work/log.txt" 2>&1) || status=$?
    echo "$t: exit $status, $(ls "$work/artifacts" | wc -l) crash files," \
        "$(ls "$work/corpus" | wc -l) corpus files"
    exit 0
fi

seconds=${1:-600}
jobs=${2:-4}
[ $# -gt 0 ] && shift
[ $# -gt 0 ] && shift

export RUSTFLAGS="-Cpasses=sancov-module -Cllvm-args=-sanitizer-coverage-level=4 \
-Cllvm-args=-sanitizer-coverage-inline-8bit-counters -Cllvm-args=-sanitizer-coverage-pc-table \
-Cllvm-args=-sanitizer-coverage-trace-compares --cfg fuzzing -Zsanitizer=address \
-Cdebug-assertions -Coverflow-checks -Cforce-frame-pointers"

cd "$here"
cargo +nightly build --release --target "$triple" --bins

if [ $# -eq 0 ]; then
    set -- $(sed -n 's/^name = "\(.*\)"$/\1/p' Cargo.toml | grep -v '^helikodu-fuzz$')
fi
for t in "$@"; do echo "$t"; done | xargs -P "$jobs" -I{} "$0" --one {} "$seconds"
