#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
# SPDX-License-Identifier: GPL-3.0-or-later

"""Accessibility smoke test of the desktop application through AT-SPI, under
headless mutter with its own accessibility bus.

    tests/ui/a11y.py --database library.db
    tests/ui/a11y.py --library target/synth-library/flac-100000-seed1

It starts the real program over a copy of the database, with the null output,
and:

1. audits every control it can reach in the sidebar, the header and search,
   the album grid, the track table, the queue panel, the player deck and
   Settings: each interactive control must have a role that says what it is
   and a non-empty accessible name; focusable objects must not be anonymous
   containers;
2. walks a keyboard-only path, typing with mutter's remote-desktop interface
   (as a keyboard would): Ctrl+F opens search, a query is typed, Tab and the
   arrow keys reach a result and Return plays it (checked over MPRIS), F6
   reaches the queue panel, Ctrl+comma opens Settings, and Space changes one
   setting (checked in settings.toml).

Needs python3-gobject with Atspi 2.0 and mutter; skips with a message when
either is missing. Prints the findings and exits non-zero when a step of the
keyboard path fails or a control lacks a name or role.
"""

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import headless  # noqa: E402

try:
    import gi

    gi.require_version("Atspi", "2.0")
    from gi.repository import Atspi, Gio, GLib
except (ImportError, ValueError):
    print("a11y: python3-gobject with Atspi 2.0 is not installed; skipped", file=sys.stderr)
    sys.exit(0)

KEYSYMS = {
    "Control_L": 0xFFE3, "Shift_L": 0xFFE1, "Alt_L": 0xFFE9, "Return": 0xFF0D,
    "Tab": 0xFF09, "Escape": 0xFF1B, "space": 0x20, "comma": 0x2C, "F6": 0xFFC3,
    "Up": 0xFF52, "Down": 0xFF54, "Left": 0xFF51, "Right": 0xFF53, "Home": 0xFF50,
}

R = Atspi.Role
INTERACTIVE = {
    R.PUSH_BUTTON, R.TOGGLE_BUTTON, R.CHECK_BOX, R.RADIO_BUTTON, R.MENU_ITEM,
    R.CHECK_MENU_ITEM, R.RADIO_MENU_ITEM, R.ENTRY, R.PASSWORD_TEXT, R.COMBO_BOX,
    R.SLIDER, R.SPIN_BUTTON, R.LINK, R.PAGE_TAB, R.LIST_ITEM, R.TABLE_CELL,
    R.TREE_ITEM, R.MENU, R.SCROLL_BAR,
}
if hasattr(R, "SWITCH"):
    INTERACTIVE.add(R.SWITCH)
# Roles that say nothing about what an object does.
ANONYMOUS = {R.PANEL, R.FILLER, R.UNKNOWN, R.REDUNDANT_OBJECT, R.SECTION, R.GROUPING,
             R.LABEL, R.IMAGE, R.INVALID}
# Scroll bars are named by their scroll pane in GTK; not counted.
NAME_OPTIONAL = {R.SCROLL_BAR}


class Keyboard:
    """Key presses through mutter's remote desktop interface."""

    def __init__(self):
        self.bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
        reply = self._call("/org/gnome/Mutter/RemoteDesktop", "org.gnome.Mutter.RemoteDesktop",
                           "CreateSession", None)
        self.session = reply.unpack()[0]
        self._call(self.session, "org.gnome.Mutter.RemoteDesktop.Session", "Start", None)
        time.sleep(0.3)

    def _call(self, path, iface, method, params):
        return self.bus.call_sync("org.gnome.Mutter.RemoteDesktop", path, iface, method, params,
                                  None, Gio.DBusCallFlags.NONE, 5000, None)

    def _sym(self, sym, down):
        self._call(self.session, "org.gnome.Mutter.RemoteDesktop.Session",
                   "NotifyKeyboardKeysym", GLib.Variant("(ub)", (sym, down)))

    def press(self, *keys, wait=0.25):
        """Presses a chord such as ("Control_L", "f")."""
        syms = [KEYSYMS.get(k, ord(k) if len(k) == 1 else 0) for k in keys]
        for s in syms:
            self._sym(s, True)
            time.sleep(0.02)
        for s in reversed(syms):
            self._sym(s, False)
            time.sleep(0.02)
        time.sleep(wait)

    def type(self, text):
        for c in text:
            self.press(c, wait=0.06)
        time.sleep(0.3)


def info(node):
    try:
        return node.get_role(), node.get_role_name(), node.get_name() or "", \
            node.get_description() or "", node.get_state_set()
    except GLib.Error:
        return None


def walk(node, path=(), out=None, limit=20000):
    """Every object below `node` with its path of (role name, name)."""
    if out is None:
        out = []
    if len(out) > limit:
        return out
    i = info(node)
    if i is None:
        return out
    role, role_name, name, desc, states = i
    here = path + ((role_name, name),)
    out.append((node, role, role_name, name, desc, states, here))
    try:
        count = node.get_child_count()
    except GLib.Error:
        count = 0
    # Hidden pages and windows carry no state worth checking; GTK keeps
    # them in the tree.
    if role not in (R.APPLICATION,) and not states.contains(Atspi.StateType.SHOWING):
        return out
    for k in range(count):
        try:
            child = node.get_child_at_index(k)
        except GLib.Error:
            continue
        if child is not None:
            walk(child, here, out, limit)
    return out


def region(path, queue_root):
    names = list(path)
    frame = names[1] if len(names) > 1 else ("", "")
    if frame[1] and frame[1] != "Helikodu":
        return frame[1]
    if ("grouping", "Player") in names:
        return "deck"
    if ("filler", "Library") in names:
        return "sidebar"
    if queue_root and names[: len(queue_root)] == list(queue_root):
        return "queue panel"
    for role, name in names:
        if role in ("table", "tree table", "list", "grid") and name:
            if name == "Albums":
                return "album grid"
            if "track" in name.lower() and "result" not in name.lower():
                return "track table"
            if "result" in name.lower() or "search" in name.lower():
                return "search"
    if any(n == "Search the library" for _, n in names):
        return "search"
    if any(r == "grouping" and n == "" for r, n in names[3:4]):
        return "content"
    return "window"


class App:
    def __init__(self, process):
        self.process = process
        self.app = None

    def find(self, timeout=30):
        end = time.time() + timeout
        while time.time() < end:
            desktop = Atspi.get_desktop(0)
            for i in range(desktop.get_child_count()):
                a = desktop.get_child_at_index(i)
                if a is not None and (a.get_name() or "").lower() == "helikodu":
                    if a.get_child_count() > 0:
                        self.app = a
                        return True
            time.sleep(0.3)
        return False

    def nodes(self):
        try:
            self.app.clear_cache()
        except (GLib.Error, AttributeError):
            pass
        return walk(self.app)

    def frames(self):
        return [self.app.get_child_at_index(i) for i in range(self.app.get_child_count())]

    def focused(self):
        for n in self.nodes():
            if n[5].contains(Atspi.StateType.FOCUSED) and n[1] not in (R.FRAME, R.WINDOW, R.DIALOG):
                return n
        return None


def mpris_status():
    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    names = bus.call_sync("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                          "ListNames", None, None, Gio.DBusCallFlags.NONE, 2000, None).unpack()[0]
    for name in names:
        if name.startswith("org.mpris.MediaPlayer2."):
            try:
                v = bus.call_sync(name, "/org/mpris/MediaPlayer2", "org.freedesktop.DBus.Properties",
                                  "Get", GLib.Variant("(ss)", ("org.mpris.MediaPlayer2.Player",
                                                               "PlaybackStatus")),
                                  None, Gio.DBusCallFlags.NONE, 2000, None)
                return v.unpack()[0]
            except GLib.Error:
                continue
    return None


def describe(n):
    if n is None:
        return "nothing"
    return f"{n[2]} {n[3]!r}"


class Findings:
    def __init__(self, dump=None):
        self.dump = dump
        self.issues = {}
        self.steps = []
        self.audited = {}

    def audit(self, app, label):
        nodes = app.nodes()
        queue_root = None
        for node, role, rn, name, desc, states, path in nodes:
            if rn == "toggle button" and name == "Queue":
                # The nearest grouping above the panel's tab strip.
                for k in range(len(path) - 1, 0, -1):
                    if path[k][0] == "grouping":
                        queue_root = path[: k + 1]
                        break
        count = 0
        for node, role, rn, name, desc, states, path in nodes:
            if not states.contains(Atspi.StateType.SHOWING):
                continue
            reg = region(path, queue_root)
            focusable = states.contains(Atspi.StateType.FOCUSABLE)
            problem = None
            if role in INTERACTIVE:
                count += 1
                if not name.strip() and role not in NAME_OPTIONAL:
                    problem = f"{rn} has no accessible name"
            elif focusable and role in ANONYMOUS and not name.strip() and role != R.LABEL:
                problem = f"focusable {rn} with no name and no meaningful role"
            if problem:
                # The nearest named list, table or window above, so that
                # repeated rows count as one finding.
                named = [f"{r} {n!r}" for r, n in path[:-1]
                         if n and r in ("list", "table", "tree table", "grid", "frame",
                                        "dialog", "grouping", "filler", "table cell",
                                        "list item")]
                context = " > ".join(named[-2:]) if named else " > ".join(r for r, _ in path[-4:-1])
                context = context if len(context) < 120 else context[:117] + "..."
                if path[-2][0] in ("table cell", "list item"):
                    context = " > ".join(named[-3:-1] + [path[-2][0]])
                children = []
                try:
                    for k in range(min(node.get_child_count(), 4)):
                        c = node.get_child_at_index(k)
                        children += [x[3] for x in walk(c) if x[3]][:2]
                except GLib.Error:
                    pass
                key = (reg, problem, context)
                entry = self.issues.setdefault(key, {"count": 0, "text": children[:3], "views": set()})
                entry["count"] += 1
                entry["views"].add(label)
        self.audited[label] = count
        if self.dump:
            with open(self.dump, "a") as d:
                d.write(f"== {label}\n")
                for node, role, rn, name, desc, states, path in nodes:
                    flags = "".join(c for c, st in (("F", Atspi.StateType.FOCUSABLE),
                                                    ("f", Atspi.StateType.FOCUSED))
                                    if states.contains(st))
                    d.write("  " * (len(path) - 1) + f"[{rn}] {name!r} {desc!r} {flags}\n")
        return nodes

    def step(self, ok, text):
        self.steps.append((ok, text))
        print(f"a11y: {'ok  ' if ok else 'FAIL'} {text}", file=sys.stderr)
        return ok


def track_results(app):
    """The names of the track results: the items after the third heading of
    the search results (Artists, Albums, Tracks)."""
    names, group = set(), 0
    for node, role, rn, name, desc, states, path in app.nodes():
        if len(path) >= 2 and path[-2] == ("list", "Search results") and role == R.LIST_ITEM:
            if not states.contains(Atspi.StateType.FOCUSABLE):
                group += 1
            elif group >= 3:
                names.add(name)
    return names


def keyboard_path(app, kb, f, profile):
    f.audit(app, "albums")
    kb.press("Control_L", "f", wait=0.6)
    n = app.focused()
    f.step(n is not None and n[1] == R.ENTRY, f"Ctrl+F focuses the search entry (focus: {describe(n)})")
    kb.type("harbour")
    time.sleep(1.5)
    f.audit(app, "search")

    # Reach a track result with Tab and the arrow keys, and play it with Return.
    tracks = track_results(app)
    played = False
    trail = []
    for _ in range(8):
        kb.press("Tab", wait=0.4)
        n = app.focused()
        trail.append(f"Tab->{describe(n)[:40]}")
        if n is not None and ("list", "Search results") in n[6]:
            break
    for _ in range(60):
        n = app.focused()
        if n is not None and n[3] in tracks:
            kb.press("Return", wait=2.0)
            status = mpris_status()
            trail.append(f"Return on {describe(n)[:50]} -> {status}")
            played = status == "Playing"
            break
        kb.press("Down", wait=0.25)
    else:
        trail.append(f"no track result reached with Down (last: {describe(app.focused())[:40]})")
    f.step(played, "a track result is reached and played from the keyboard: " + "; ".join(trail))

    # The queue panel through F6.
    reached = False
    trail = []
    for _ in range(5):
        kb.press("F6", wait=0.5)
        n = app.focused()
        nodes = app.nodes()
        queue_root = None
        for node, role, rn, name, desc, states, path in nodes:
            if rn == "toggle button" and name == "Queue":
                for k in range(len(path) - 1, 0, -1):
                    if path[k][0] == "grouping":
                        queue_root = path[: k + 1]
                        break
        reg = region(n[6], queue_root) if n else None
        trail.append(f"F6->{reg}:{describe(n)[:40]}")
        if reg == "queue panel":
            reached = True
            break
    f.step(reached, "F6 reaches the queue panel: " + "; ".join(trail))
    f.audit(app, "queue")

    # Settings: open, change one switch with Space, close.
    settings_file = profile.root / "config" / "helikodu" / "settings.toml"
    before = settings_file.read_text() if settings_file.exists() else ""
    kb.press("Control_L", "comma", wait=1.5)
    frames = [fr.get_name() for fr in app.frames()]
    opened = len(frames) > 1
    f.step(opened, f"Ctrl+comma opens Settings (windows: {frames})")
    if opened:
        f.audit(app, "settings")
        toggles = (R.CHECK_BOX,) + ((R.SWITCH,) if hasattr(R, "SWITCH") else ())
        trail = []
        for _ in range(40):
            kb.press("Tab", wait=0.3)
            n = app.focused()
            trail.append(describe(n)[:30])
            if n is not None and n[1] in toggles:
                break
        changed = False
        if n is not None and n[1] in toggles:
            for key in ("space", "Return"):
                was = n[5].contains(Atspi.StateType.CHECKED)
                status_before = mpris_status()
                kb.press(key, wait=1.5)
                after = app.focused()
                now = after is not None and after[5].contains(Atspi.StateType.CHECKED)
                text = settings_file.read_text() if settings_file.exists() else ""
                status_after = mpris_status()
                trail.append(f"{key} on {describe(n)}: checked {was}->{now}, settings.toml "
                             f"{'changed' if text != before else 'unchanged'}, playback "
                             f"{status_before}->{status_after}")
                if now != was and text != before:
                    changed = key == "space"
                    break
        f.step(changed, "a setting is changed with Space and saved: " + " | ".join(trail[-4:]))
        # Every page of Settings, through the page list.
        for _ in range(60):
            n = app.focused()
            if n is not None and ("list", "Settings pages") in n[6]:
                break
            kb.press("Shift_L", "Tab", wait=0.25)
        kb.press("Home", wait=0.5)
        for _ in range(9):
            f.audit(app, "settings")
            kb.press("Down", wait=0.6)
        kb.press("Escape", wait=1.0)
        frames = [fr.get_name() for fr in app.frames()]
        active = [fr.get_name() for fr in app.frames()
                  if fr.get_state_set().contains(Atspi.StateType.ACTIVE)]
        f.step(len(frames) == 1, f"Escape closes Settings (windows: {frames})")
        f.step(bool(active), f"the main window is active again after Settings closes "
                             f"(active: {active}; focus: {describe(app.focused())[:40]})")

    # The track table, from the sidebar.
    trail = []
    for _ in range(5):
        kb.press("F6", wait=0.4)
        n = app.focused()
        if n and ("filler", "Library") in n[6]:
            break
    opened = False
    for attempt in ("Down", "Tab"):
        if attempt == "Tab":
            # Arrow keys did not enter the rows; try Tab into them.
            kb.press("Tab", wait=0.4)
        for _ in range(8):
            n = app.focused()
            trail.append(f"{attempt}:{describe(n)[:30]}")
            names = [x[3] for x in walk(n[0])] if n and n[1] == R.LIST_ITEM else []
            if "Tracks" in names:
                kb.press("Return", wait=3.0)
                opened = True
                break
            kb.press("Down", wait=0.3)
        if opened:
            break
    nodes = f.audit(app, "tracks")
    shown = any(rn in ("table", "tree table") and "track" in name.lower()
                and s.contains(Atspi.StateType.SHOWING)
                for _, _, rn, name, _, s, _ in nodes)
    f.step(shown, "the sidebar opens Tracks from the keyboard: " + " | ".join(trail))


def main():
    headless.in_private_session(sys.argv)
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("--library")
    p.add_argument("--database")
    p.add_argument("--target-dir")
    p.add_argument("--no-build", action="store_true")
    p.add_argument("--dump", help="append the accessibility tree of each audited view here")
    args = p.parse_args()
    if shutil.which("mutter") is None:
        print("a11y: mutter is not installed (keyboard input needs its remote desktop); skipped",
              file=sys.stderr)
        sys.exit(0)
    target = Path(args.target_dir) if args.target_dir else headless.REPO / "target"
    if not args.no_build:
        headless.cargo_build(["-p", "helikodu", "-p", "helikodu-cli", "-p", "helikodu-scan-worker"],
                             args.target_dir)
    database, tmp = args.database, None
    if args.library:
        tmp = Path(tempfile.mkdtemp(prefix="helikodu-a11y-db-"))
        database = tmp / "library.db"
        subprocess.check_call([str(target / "release" / "helikodu-cli"), "scan", "--database",
                               str(database), args.library], stdout=subprocess.DEVNULL)
    if not database:
        sys.exit("a11y: give --library or --database")
    profile = headless.Profile(database)
    f = Findings(args.dump)
    try:
        with headless.Compositor() as comp:
            if comp.bus is None:
                print("a11y: no AT-SPI bus launcher; skipped", file=sys.stderr)
                sys.exit(0)
            Atspi.init()
            env = comp.env(**profile.env())
            env.pop("GTK_A11Y", None)
            process = subprocess.Popen([str(target / "release" / "helikodu"), "--scheme", "dark"],
                                       env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            app = App(process)
            try:
                if not f.step(app.find(), "the application appears on the accessibility bus"):
                    sys.exit(1)
                time.sleep(2)
                kb = Keyboard()
                keyboard_path(app, kb, f, profile)
            finally:
                process.terminate()
                try:
                    process.wait(10)
                except subprocess.TimeoutExpired:
                    process.kill()
    finally:
        profile.remove()
        if tmp:
            shutil.rmtree(tmp, ignore_errors=True)

    print("## Keyboard path\n")
    for ok, text in f.steps:
        print(f"- {'pass' if ok else 'FAIL'}: {text}")
    print("\n## Controls audited per view\n")
    for label, count in f.audited.items():
        print(f"- {label}: {count} interactive objects")
    print("\n## Missing names or roles\n")
    if not f.issues:
        print("None.")
    for (reg, problem, context), e in sorted(f.issues.items()):
        text = f"; content: {', '.join(repr(t) for t in e['text'])}" if e["text"] else ""
        print(f"- [{reg}] {problem} ×{e['count']} (in {context}; seen in {', '.join(sorted(e['views']))}){text}")
    failed = any(not ok for ok, _ in f.steps) or bool(f.issues)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
