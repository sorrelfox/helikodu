#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
# SPDX-License-Identifier: GPL-3.0-or-later

"""Start-up, memory, scrolling and idle budgets
of the desktop application over a large library, under a headless compositor.

    tests/ui/perf.py --library target/synth-library/flac-100000-seed1
    tests/ui/perf.py --database library.db --idle-secs 60

With `--library` the folder is scanned with `helikodu-cli scan` into a
temporary database first. The program measured is the `perf` example of
helikodu-ui-gtk, which starts the front end exactly as the application does
and plays only to the null output.

Runs: one cold start (the database, the programs and the profile's caches
dropped from the page cache, an empty thumbnail cache, no saved session), then
warm starts; the last warm run also scrolls and measures idle CPU. Prints a
table against the budgets and exits non-zero when one is missed.
"""

import argparse
import json
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import headless  # noqa: E402

# The budgets of the requirements.
FIRST_FRAME_MS = 1000.0
INTERACTIVE_MS = 1500.0
MAX_FRAME_MS = 50.0
RSS_MB = 250.0  # megabytes, 10^6 bytes
IDLE_CPU_PCT = 0.1
# "At the display refresh rate": the 95th percentile interval within 10 % of
# one refresh (60 Hz on the virtual output).
REFRESH_MS = 1000.0 / 60.0


def parse():
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("--library", help="a music folder to scan into a temporary database")
    p.add_argument("--database", help="an existing library database (copied, never changed)")
    p.add_argument("--warm-runs", type=int, default=3, help="warm start-ups (default 3)")
    p.add_argument("--idle-secs", type=int, default=60)
    p.add_argument("--scroll-secs", type=float, default=5.0)
    p.add_argument("--target-dir", help="Cargo target folder for the build")
    p.add_argument("--no-build", action="store_true", help="use the programs already built")
    p.add_argument("--json", help="also write the raw results here")
    p.add_argument("--from-json", help="only print the report of earlier raw results")
    return p.parse_args()


def measure(args, target, profile):
    perf = target / "release" / "examples" / "perf"
    raw = Path(tempfile.mkstemp(prefix="helikodu-perf-", suffix=".jsonl")[1])
    results = []
    with headless.Compositor() as comp:
        runs = [("cold", True)] + [(f"warm{i + 1}", False) for i in range(args.warm_runs)]
        for n, (label, cold) in enumerate(runs):
            last = n == len(runs) - 1
            if cold:
                profile.reset_cache_and_state()
                dropped = headless.evict(
                    [profile.root, perf, *headless.shared_libraries(perf)]
                )
                print(f"perf: cold start, {dropped} files dropped from the page cache", file=sys.stderr)
                time.sleep(1)
            cmd = [str(perf), "--out", str(raw), "--label", label,
                   "--idle-secs", str(args.idle_secs), "--scroll-secs", str(args.scroll_secs)]
            if not last:
                cmd.append("--startup-only")
            env = comp.env(**profile.env())
            launched = time.monotonic_ns() // 1000
            cmd += ["--launched-us", str(launched)]
            print(f"perf: run {label}{' (scroll and idle)' if last else ''}", file=sys.stderr)
            proc = subprocess.run(cmd, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                                  text=True, timeout=300 + 6 * args.idle_secs)
            if proc.returncode != 0:
                print(proc.stderr[-2000:], file=sys.stderr)
            time.sleep(1)
    for line in raw.read_text().splitlines():
        results.append(json.loads(line))
    raw.unlink()
    return results


def rss_without_driver(full, prefix):
    """The RSS in MB less the resident pages of the GL driver's libraries."""
    rss = full.get(prefix.replace("mem_", "rss_") + "_mib", 0.0)
    return (rss - full.get(f"{prefix}.gl_driver_mib", 0.0)) * 2**20 / 1e6


def verdict(ok):
    return "pass" if ok else "FAIL"


def report(results):
    rows = []
    failed = False

    def add(what, value, budget, ok, note=""):
        nonlocal failed
        if ok is False:
            failed = True
        state = "info" if ok is None else verdict(ok)
        rows.append((what, value, budget, state, note))

    cold = next((r for r in results if r["label"] == "cold"), None)
    warm = [r for r in results if r["label"].startswith("warm")]
    full = results[-1] if results else {}

    def fmt(v, unit="ms"):
        return "n/a" if v is None else f"{v:.0f} {unit}" if unit == "ms" else f"{v:.1f} {unit}"

    if cold:
        add("Cold start, first frame", fmt(cold.get("startup.first_frame_ms")), "(warm budget 1.0 s)", None)
        add("Cold start, first frame with albums", fmt(cold.get("startup.first_frame_with_albums_ms")),
            "(warm budget 1.0 s)", None)
    if warm:
        ff = statistics.median(r.get("startup.first_frame_with_albums_ms", float("inf")) for r in warm)
        ia = statistics.median(r.get("startup.interactive_ms", float("inf")) for r in warm)
        f0 = statistics.median(r.get("startup.first_frame_ms", float("inf")) for r in warm)
        add("Warm start, first frame (median)", fmt(f0), "≤ 1000 ms", f0 <= FIRST_FRAME_MS)
        add("Warm start, first frame with albums (median)", fmt(ff), "≤ 1000 ms", ff <= FIRST_FRAME_MS)
        add("Warm start, interactive (median)", fmt(ia), "≤ 1500 ms", ia <= INTERACTIVE_MS)
    mib_budget = RSS_MB * 1e6 / 2**20
    for key, what in [("rss_after_startup_mib", "RSS after start-up"),
                      ("rss_after_albums_scroll_mib", "RSS after scrolling albums"),
                      ("rss_after_tracks_open_mib", "RSS after opening tracks"),
                      ("rss_after_tracks_scroll_mib", "RSS after scrolling tracks"),
                      ("rss_peak_mib", "RSS peak")]:
        v = full.get(key)
        if v is not None:
            add(what, f"{v:.0f} MiB ({v * 2**20 / 1e6:.0f} MB)", "≤ 250 MB", v <= mib_budget)
    # Where the memory is (/proc/self/smaps_rollup): pages only this process
    # maps and pages shared with others, the proportional set size, anonymous
    # memory, and the graphics driver's libraries (private when no other
    # process maps them).
    for prefix, what in [("mem_after_startup", "after start-up"),
                         ("mem_after_albums_scroll", "after scrolling albums"),
                         ("mem_after_tracks_scroll", "after scrolling tracks")]:
        if f"{prefix}.private_mib" in full:
            add(f"Memory split {what}",
                f"private {full[f'{prefix}.private_mib']:.0f} MiB, "
                f"shared {full[f'{prefix}.shared_mib']:.0f} MiB, "
                f"PSS {full[f'{prefix}.pss_mib']:.0f} MiB",
                "", None,
                f"anonymous {full[f'{prefix}.anon_mib']:.0f} MiB; "
                f"GL driver libraries {full.get(f'{prefix}.gl_driver_mib', 0):.0f} MiB; "
                f"RSS without them {rss_without_driver(full, prefix):.0f} MB")
    for when, what in [("after_startup", "after start-up"),
                       ("after_tracks_scroll", "after scrolling tracks")]:
        if f"widgets_{when}" in full:
            add(f"Widgets {what}",
                f"{full[f'widgets_{when}']:.0f}, {full[f'widgets_{when}.mapped']:.0f} shown",
                "", None, full.get(f"widgets_{when}.top", ""))
    for view in ("albums", "tracks"):
        for mode in ("slow", "fast", "jump"):
            k = f"{view}.{mode}"
            if f"{k}.frames" not in full:
                add(f"Scroll {view}, {mode}", "not measured", "", False, full.get(f"{view}.error", ""))
                continue
            p95, p99, mx = full[f"{k}.interval_p95_ms"], full[f"{k}.interval_p99_ms"], full[f"{k}.interval_max_ms"]
            # Random jumps (dragging the scrollbar) are held to the 50 ms
            # limit only; steady scrolling also to the refresh rate.
            steady = mode != "jump"
            ok = mx <= MAX_FRAME_MS and (not steady or p95 <= REFRESH_MS * 1.1)
            add(f"Scroll {view} ({mode}) frame interval p95/p99/max",
                f"{p95:.1f}/{p99:.1f}/{mx:.1f} ms",
                "refresh rate; none > 50 ms" if steady else "none > 50 ms", ok,
                f"{full[f'{k}.frames']:.0f} frames; work p95/p99/max "
                f"{full[f'{k}.work_p95_ms']:.1f}/{full[f'{k}.work_p99_ms']:.1f}/{full[f'{k}.work_max_ms']:.1f} ms")
    for phase, what in [("idle_stopped", "Idle CPU, stopped"), ("idle_paused", "Idle CPU, paused")]:
        if f"{phase}.cpu_threads_pct" in full:
            v = full[f"{phase}.cpu_process_pct"]
            painted = full[f"{phase}.frames_painted"]
            # "No periodic work": the frame clock must stop as well.
            add(what, f"{v:.3f} % of a core; {painted:.0f} frames in "
                f"{full[f'{phase}.seconds']:.0f} s", "< 0.1 %, no periodic work",
                v < IDLE_CPU_PCT and painted == 0,
                f"main thread {full[f'{phase}.cpu_main_pct']:.3f} %, "
                f"{full[f'{phase}.main_wakeups_per_s']:.1f} main-thread wake-ups/s")
        else:
            add(what, "not measured", "< 0.1 %", False)
    for phase, what in [("playing_hidden", "Playing, window hidden"),
                        ("playing_minimized", "Playing, window minimized")]:
        if f"{phase}.frames_painted" in full:
            painted = full[f"{phase}.frames_painted"]
            add(what, f"{painted:.0f} frames painted; CPU {full[f'{phase}.cpu_process_pct']:.2f} %",
                "no drawing work", painted == 0,
                f"main thread {full[f'{phase}.cpu_main_pct']:.3f} %, "
                f"{full[f'{phase}.main_wakeups_per_s']:.1f} main-thread wake-ups/s"
                + (f"; busiest threads: {full[f'{phase}.busiest_threads']}"
                   if full.get(f"{phase}.busiest_threads") else ""))
        else:
            add(what, "not measured", "no drawing work", False)
    if "tracks.keys_loaded_ms" in full:
        add("Tracks view: all track keys loaded", fmt(full["tracks.keys_loaded_ms"]), "", None,
            f"{full.get('library_tracks', 0):.0f} tracks")
    if "playing_shown.cpu_threads_pct" in full:
        add("Playing, window shown (reference)",
            f"{full['playing_shown.cpu_process_pct']:.2f} % CPU, "
            f"{full['playing_shown.frames_painted']:.0f} frames in 10 s", "", None)

    print("| Check | Measured | Budget | Result | Notes |")
    print("|---|---|---|---|---|")
    for r in rows:
        print("| " + " | ".join(str(c) for c in r) + " |")
    return failed


def main():
    args = parse()
    if args.from_json:
        sys.exit(1 if report(json.loads(Path(args.from_json).read_text())) else 0)
    headless.in_private_session(sys.argv)
    target = Path(args.target_dir) if args.target_dir else headless.REPO / "target"
    if not args.no_build:
        headless.cargo_build(["-p", "helikodu-cli", "-p", "helikodu-scan-worker",
                              "-p", "helikodu-ui-gtk", "--example", "perf"], args.target_dir)
    database = args.database
    tmp = None
    if args.library:
        tmp = Path(tempfile.mkdtemp(prefix="helikodu-perf-db-"))
        database = tmp / "library.db"
        cli = target / "release" / "helikodu-cli"
        print(f"perf: scanning {args.library}", file=sys.stderr)
        t0 = time.monotonic()
        subprocess.check_call([str(cli), "scan", "--database", str(database), args.library],
                              stdout=subprocess.DEVNULL)
        print(f"perf: scanned in {time.monotonic() - t0:.1f} s", file=sys.stderr)
    if not database:
        sys.exit("perf: give --library or --database")
    profile = headless.Profile(database)
    try:
        results = measure(args, target, profile)
    finally:
        profile.remove()
        if tmp:
            import shutil
            shutil.rmtree(tmp, ignore_errors=True)
    if args.json:
        Path(args.json).write_text(json.dumps(results, indent=1))
    failed = report(results)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
