# SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
# SPDX-License-Identifier: GPL-3.0-or-later

"""Shared helpers of the UI checks: a headless compositor on a private D-Bus
session, a test profile with the null output, and the built programs.

The checks re-run themselves inside `dbus-run-session`, so nothing they start
(the compositor, the accessibility bus, the program under test) touches the
desktop session they are started from.
"""

import os
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
INNER = "HELIKODU_UI_CHECK_INNER"


def in_private_session(argv):
    """Re-runs this script inside a private D-Bus session unless it already is."""
    if os.environ.get(INNER) == "1":
        return
    if shutil.which("dbus-run-session") is None:
        sys.exit("dbus-run-session is not installed")
    env = dict(os.environ, **{INNER: "1"})
    env.pop("DBUS_SESSION_BUS_ADDRESS", None)
    status = subprocess.call(
        ["dbus-run-session", "--", sys.executable, *argv], env=env
    )
    sys.exit(status)


class Compositor:
    """`mutter --headless` (or KWin's virtual backend) with one 1920x1080 output."""

    def __init__(self, size="1920x1080", a11y=True):
        self.size = size
        self.a11y = a11y
        self.process = None
        self.bus = None
        self.socket = f"helikodu-ui-{os.getpid()}"

    def __enter__(self):
        runtime = os.environ.get("XDG_RUNTIME_DIR", "/tmp")
        w, h = self.size.split("x")
        if shutil.which("mutter"):
            cmd = [
                "mutter", "--headless", "--no-x11",
                "--virtual-monitor", f"{self.size}@60",
                f"--wayland-display={self.socket}",
            ]
            self.kind = "mutter"
        elif shutil.which("kwin_wayland"):
            cmd = [
                "kwin_wayland", "--virtual", "--no-lockscreen",
                "--width", w, "--height", h, "--socket", self.socket,
            ]
            self.kind = "kwin"
        else:
            sys.exit("neither mutter nor kwin_wayland is installed")
        self.process = subprocess.Popen(
            cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
        )
        path = Path(runtime) / self.socket
        for _ in range(200):
            if path.exists():
                break
            time.sleep(0.05)
        else:
            raise RuntimeError("the headless compositor did not start")
        time.sleep(0.5)
        if self.a11y:
            self.bus = start_a11y_bus()
        return self

    def env(self, **extra):
        env = dict(os.environ)
        env.pop("DISPLAY", None)
        env.update(
            WAYLAND_DISPLAY=self.socket,
            GDK_BACKEND="wayland",
        )
        env.update(extra)
        return env

    def __exit__(self, *exc):
        if self.bus:
            self.bus.terminate()
        if self.process:
            self.process.terminate()
            try:
                self.process.wait(5)
            except subprocess.TimeoutExpired:
                self.process.kill()


def start_a11y_bus():
    """Starts the accessibility bus in the private session, as a desktop does.
    D-Bus activation of it can be refused (SELinux, no systemd user manager),
    so it is started directly. Returns the process, or None."""
    for launcher in ("/usr/libexec/at-spi-bus-launcher", "/usr/lib/at-spi-bus-launcher",
                     "/usr/lib/at-spi2-core/at-spi-bus-launcher"):
        if os.path.exists(launcher):
            break
    else:
        return None
    process = subprocess.Popen([launcher, "--launch-immediately"],
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    address = None
    for _ in range(50):
        probe = subprocess.run(
            ["gdbus", "call", "--session", "-d", "org.a11y.Bus", "-o", "/org/a11y/bus",
             "-m", "org.a11y.Bus.GetAddress"], capture_output=True, text=True)
        if probe.returncode == 0:
            address = probe.stdout.strip().strip("(),'")
            break
        time.sleep(0.1)
    # The registry is activated on the accessibility bus through systemd, which
    # can fail the same way; start it directly too.
    for registry in ("/usr/libexec/at-spi2-registryd", "/usr/lib/at-spi2-registryd",
                     "/usr/lib/at-spi2-core/at-spi2-registryd"):
        if address and os.path.exists(registry):
            env = dict(os.environ, AT_SPI_BUS_ADDRESS=address)
            subprocess.Popen([registry], env=env, stdout=subprocess.DEVNULL,
                             stderr=subprocess.DEVNULL, start_new_session=False)
            time.sleep(0.3)
            break
    return process


def copy_database(source, target):
    """Copies a library database with what is still in its write-ahead log:
    a plain file copy leaves out the changes not yet written back, and the
    copy can then look empty."""
    src = sqlite3.connect(f"file:{source}?mode=ro", uri=True)
    try:
        dst = sqlite3.connect(target)
        try:
            src.backup(dst)
        finally:
            dst.close()
    finally:
        src.close()


class Profile:
    """A throw-away home for the program: XDG folders with settings that select
    the null output, and optionally a copy of a library database."""

    def __init__(self, database=None, keep=None):
        self.root = Path(keep) if keep else Path(tempfile.mkdtemp(prefix="helikodu-ui-"))
        self.database = database
        for sub in ("config", "data", "state", "cache"):
            (self.root / sub / "helikodu").mkdir(parents=True, exist_ok=True)
        settings = self.root / "config" / "helikodu" / "settings.toml"
        settings.write_text('[output]\nbackend = "null"\n')
        if database:
            target = self.root / "data" / "helikodu" / "library.db"
            if not target.exists():
                copy_database(database, target)
        self.library = self.root / "data" / "helikodu" / "library.db"

    def env(self):
        return {
            "HOME": str(self.root),
            "XDG_CONFIG_HOME": str(self.root / "config"),
            "XDG_DATA_HOME": str(self.root / "data"),
            "XDG_STATE_HOME": str(self.root / "state"),
            "XDG_CACHE_HOME": str(self.root / "cache"),
        }

    def reset_cache_and_state(self):
        for sub in ("state", "cache"):
            shutil.rmtree(self.root / sub / "helikodu", ignore_errors=True)
            (self.root / sub / "helikodu").mkdir(parents=True, exist_ok=True)

    def remove(self):
        shutil.rmtree(self.root, ignore_errors=True)


def cargo_build(args, target_dir=None):
    """Builds with Cargo and returns the target folder."""
    env = dict(os.environ)
    if target_dir:
        env["CARGO_TARGET_DIR"] = str(target_dir)
    subprocess.check_call(["cargo", "build", "--release", *args], cwd=REPO, env=env)
    return Path(target_dir) if target_dir else REPO / "target"


def evict(paths):
    """Drops the page cache of the files under `paths` (no root needed; pages
    still mapped by other processes stay)."""
    count = 0
    for top in paths:
        top = Path(top)
        files = [top] if top.is_file() else [p for p in top.rglob("*") if p.is_file()]
        for f in files:
            try:
                fd = os.open(f, os.O_RDONLY)
            except OSError:
                continue
            try:
                os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
                count += 1
            finally:
                os.close(fd)
    return count


def shared_libraries(binary):
    """The shared libraries `binary` loads, from ldd."""
    try:
        out = subprocess.check_output(["ldd", str(binary)], text=True)
    except (OSError, subprocess.CalledProcessError):
        return []
    libs = []
    for line in out.splitlines():
        parts = line.split("=>")
        target = (parts[1] if len(parts) > 1 else parts[0]).strip().split(" (")[0]
        if target.startswith("/"):
            libs.append(target)
    return libs
