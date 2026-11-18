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

3. audits every other window and view the same way: the sidebar views and
   the pages they open, the facet browser, the tag editor and Tag tools,
   playlists and the smart playlist editor, History, Statistics, query
   search, Now Playing and the analyzer, the menus and popovers, Settings
   with its search, and every dialog and tool window, opened through the
   application's actions on the session bus (org.gtk.Actions) as their menu
   items do; in each, Tab must keep the focus on a named object of the
   window and must move it;
4. serves a StatusNotifierWatcher so that the tray icon registers, and reads
   the tray menu over D-Bus as a panel would: every item needs a label.

    tests/ui/a11y.py --database library.db --skip-path --views "convert,view Home"

Needs python3-gobject with Atspi 2.0 and mutter; skips with a message when
either is missing. Prints the findings and exits non-zero when a step fails,
a control lacks a name or role, or Tab loses or traps the focus.
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
    "End": 0xFF57, "F4": 0xFFC1, "F10": 0xFFC7, "F11": 0xFFC8, "colon": 0x3A,
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


def empty_cell(node, role):
    """A table cell that shows nothing (an unknown artist, say) needs no name."""
    if role != R.TABLE_CELL:
        return False
    return not any(x[3].strip() for x in walk(node)[1:])


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

    def active_frame(self):
        """The name of the window in front, or None."""
        for fr in self.frames():
            try:
                if fr.get_state_set().contains(Atspi.StateType.ACTIVE):
                    return fr.get_name() or ""
            except GLib.Error:
                continue
        return None

    def focused(self):
        """The focused object of the window in front (of any window when none
        says it is in front)."""
        active = self.active_frame()
        found = None
        for n in self.nodes():
            if n[5].contains(Atspi.StateType.FOCUSED) and n[1] not in (R.FRAME, R.WINDOW, R.DIALOG):
                frame = n[6][1][1] if len(n[6]) > 1 else None
                if active is None or frame == active:
                    return n
                found = found or n
        return found if active is None else None


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
        self.tab_stops = {}

    def problem(self, label, reg, text):
        """A finding that is not about one object: focus, the tray."""
        key = (reg, text, label)
        entry = self.issues.setdefault(key, {"count": 0, "text": [], "views": set()})
        entry["count"] += 1
        entry["views"].add(label)

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
                if not name.strip() and role not in NAME_OPTIONAL and not empty_cell(node, role):
                    problem = f"{rn} has no accessible name"
            elif focusable and role in ANONYMOUS and not name.strip() and role != R.LABEL:
                problem = f"focusable {rn} with no name and no meaningful role"
            if problem is None and len(desc) > 300:
                # GTK makes a description from what a widget holds, such as
                # the rows of a popover: read in full, it buries the control.
                problem = f"{rn} has a description of {len(desc)} characters"
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
    """The names of the track results: the items after the "Tracks" heading of
    the search results (the Artists and Albums headings are left out when
    nothing matches there)."""
    names, in_tracks = set(), False
    for node, role, rn, name, desc, states, path in app.nodes():
        if len(path) >= 2 and path[-2] == ("list", "Search results") and role == R.LIST_ITEM:
            if not states.contains(Atspi.StateType.FOCUSABLE):
                in_tracks = name.startswith("Tracks")
            elif in_tracks:
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


# --- Every window and view ---------------------------------------------------

APP_ID = "io.github.sorrelfox.Helikodu"
APP_PATH = "/io/github/sorrelfox/Helikodu"


class Actions:
    """The application's and the main window's actions, which GTK exports on
    the session bus (org.gtk.Actions). Opening a window this way is what its
    menu item does; the keyboard path above covers reaching the menus."""

    def __init__(self):
        self.bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
        self.window_path = None
        for k in range(1, 6):
            path = f"{APP_PATH}/window/{k}"
            try:
                self._call(path, "List", None)
                self.window_path = path
                break
            except GLib.Error:
                continue

    def _call(self, path, method, params):
        return self.bus.call_sync(APP_ID, path, "org.gtk.Actions", method, params, None,
                                  Gio.DBusCallFlags.NONE, 5000, None)

    def names(self, scope):
        path = APP_PATH if scope == "app" else self.window_path
        try:
            return set(self._call(path, "List", None).unpack()[0])
        except (GLib.Error, TypeError):
            return set()

    def activate(self, scope, name, parameter=None):
        path = APP_PATH if scope == "app" else self.window_path
        params = [parameter] if parameter is not None else []
        try:
            self._call(path, "Activate", GLib.Variant("(sava{sv})", (name, params, {})))
            return True
        except (GLib.Error, TypeError) as error:
            print(f"a11y: cannot activate {scope}.{name}: {error}", file=sys.stderr)
            return False


def wait_for(check, timeout=5.0, step=0.2):
    end = time.time() + timeout
    while time.time() < end:
        value = check()
        if value:
            return value
        time.sleep(step)
    return check()


def frame_names(app):
    try:
        app.app.clear_cache()
    except (GLib.Error, AttributeError):
        pass
    names = []
    for fr in app.frames():
        try:
            names.append(fr.get_name() or "")
        except (GLib.Error, AttributeError):
            continue
    return names


def tab_walk(app, kb, f, label, presses=24, frame=None):
    """Tabs through the window in front: focus must stay on a named object of
    that window and must move. Records the stops."""
    stops, lost, outside = [], 0, []
    for _ in range(presses):
        kb.press("Tab", wait=0.25)
        n = app.focused()
        if n is None:
            lost += 1
            continue
        here = n[6][1][1] if len(n[6]) > 1 else ""
        if frame is not None and here != frame:
            outside.append(describe(n)[:40])
        stops.append((n[2], n[3]))
    distinct = list(dict.fromkeys(stops))
    f.tab_stops[label] = len(distinct)
    if lost:
        f.problem(label, "focus", f"focus was on nothing after {lost} of {presses} Tab presses")
    if outside:
        f.problem(label, "focus", f"Tab left the window: {', '.join(outside[:3])}")
    if stops and len(distinct) < 2:
        f.problem(label, "focus", f"Tab does not move focus (stays on {distinct[0][0]} "
                                  f"{distinct[0][1]!r})")
    return distinct


def close_front(app, kb, base):
    """Closes the window in front with Escape, or Alt+F4 when Escape does not."""
    for keys in (("Escape",), ("Alt_L", "F4")):
        if len(frame_names(app)) <= base:
            return True
        kb.press(*keys, wait=0.8)
        wait_for(lambda: len(frame_names(app)) <= base, timeout=2.0)
    return len(frame_names(app)) <= base


def audit_window(app, kb, f, label, open_it, tab=True, after=None):
    """Opens a window, audits it, tabs through it and closes it."""
    base = len(frame_names(app))
    if not open_it():
        f.step(False, f"{label}: could not be opened")
        return False
    opened = wait_for(lambda: len(frame_names(app)) > base, timeout=6.0)
    if not opened:
        f.step(False, f"{label} opens (windows: {frame_names(app)})")
        return False
    time.sleep(0.8)
    name = app.active_frame()
    f.audit(app, label)
    if tab:
        tab_walk(app, kb, f, label, frame=name)
    if after:
        after(name)
    closed = close_front(app, kb, base)
    f.step(closed, f"{label} opens and closes from the keyboard (window {name!r})")
    return True


def go_sidebar(app, kb, item):
    """Opens the sidebar item `item` with F6, the arrow keys and Return."""
    for _ in range(6):
        n = app.focused()
        if n is not None and ("list", "Library") in n[6]:
            break
        kb.press("F6", wait=0.4)
    else:
        return False
    kb.press("Home", wait=0.3)
    for _ in range(40):
        n = app.focused()
        if n is not None and n[1] == R.LIST_ITEM and n[3] == item:
            kb.press("Return", wait=1.5)
            return True
        kb.press("Down", wait=0.2)
    return False


def focus_content(app, kb):
    """Moves focus into the content pane with F6."""
    for _ in range(6):
        kb.press("F6", wait=0.4)
        n = app.focused()
        if n is not None and ("filler", "Library") not in n[6] \
                and ("grouping", "Player") not in n[6] and region(n[6], None) != "queue panel":
            return n
    return None


def popover(app, kb, f, label, open_it):
    """Opens a menu or popover, audits it and closes it with Escape. A popover
    is open when it took the focus."""
    before = app.focused()
    if not open_it():
        f.step(False, f"{label}: could not be opened")
        return
    time.sleep(0.8)
    f.audit(app, label)
    after = app.focused()
    moved = after is not None and (before is None or after[6] != before[6])
    kb.press("Escape", wait=0.6)
    f.step(moved, f"{label} opens and takes the focus (focus: {describe(after)[:50]})")


def sweep(app, kb, f, views):
    """Audits every window and view the keyboard path does not reach."""
    actions = Actions()
    app_actions, win_actions = actions.names("app"), actions.names("win")
    want = (lambda name: True) if not views else (lambda name: name in views)

    def act(scope, name, parameter=None):
        available = app_actions if scope == "app" else win_actions
        if name not in available:
            print(f"a11y: no action {scope}.{name}", file=sys.stderr)
            return False
        return lambda: actions.activate(scope, name, parameter)

    # The views of the sidebar, each audited and tabbed through.
    for item in ("Home", "Albums", "Artists", "Tracks", "Genres", "Composers", "Years",
                 "Folders"):
        label = f"view {item}"
        if not want(label):
            continue
        reached = go_sidebar(app, kb, item)
        f.step(reached, f"the sidebar opens {item} from the keyboard")
        if not reached:
            continue
        time.sleep(1.0)
        f.audit(app, label)
        if focus_content(app, kb):
            tab_walk(app, kb, f, label, presses=12, frame="Helikodu")
        if item in ("Albums", "Artists", "Genres", "Folders"):
            # The first entry's page: an album, an artist (with the photo
            # menu), a genre, a folder.
            n = focus_content(app, kb)
            if n is not None:
                kb.press("Down", wait=0.3)
                kb.press("Return", wait=1.5)
                f.audit(app, f"{label} › first entry")
                kb.press("Alt_L", "Left", wait=1.0)
        if item == "Tracks":
            # The facet browser, and the tag editor for the first track.
            n = focus_content(app, kb)
            for _ in range(10):
                n = app.focused()
                if n is not None and n[2] == "toggle button" and "facet" in (n[3] + n[4]).lower():
                    kb.press("space", wait=1.0)
                    f.audit(app, "facet browser")
                    kb.press("space", wait=0.6)
                    break
                kb.press("Tab", wait=0.2)
            if want("tag editor"):
                focus_content(app, kb)
                # Select the first track: the table takes focus from F6.
                kb.press("Down", wait=0.3)
                kb.press("Up", wait=0.3)

                def tag_tools(frame):
                    for _ in range(40):
                        n = app.focused()
                        if n is not None and n[3] == "Tag tools":
                            kb.press("Return", wait=0.8)
                            f.audit(app, "tag tools menu")
                            base = len(frame_names(app))
                            kb.press("Return", wait=1.5)
                            if len(frame_names(app)) > base:
                                f.audit(app, "tag tools")
                                tab_walk(app, kb, f, "tag tools", frame=app.active_frame())
                                f.step(close_front(app, kb, base), "Tag tools close with Escape")
                            return
                        kb.press("Tab", wait=0.2)
                    f.step(False, "the tag editor's Tag tools button is reached with Tab")

                audit_window(app, kb, f, "tag editor", act("win", "track-properties"),
                             after=tag_tools)

    # Playlists: a new playlist and the smart playlist editor.
    if want("new playlist") and act("app", "new-playlist"):
        actions.activate("app", "new-playlist")
        time.sleep(1.5)
        f.audit(app, "new playlist")
    if want("smart playlist editor") and act("win", "new-smart-playlist-in"):
        actions.activate("win", "new-smart-playlist-in", GLib.Variant("t", 0))
        time.sleep(1.5)
        f.audit(app, "smart playlist editor")
        if focus_content(app, kb):
            tab_walk(app, kb, f, "smart playlist editor", presses=16, frame="Helikodu")

    # Views opened from the menus.
    for label, scope, name in (("history", "win", "show-history"),
                               ("statistics", "win", "show-statistics")):
        if want(label) and act(scope, name):
            actions.activate(scope, name)
            time.sleep(1.5)
            f.audit(app, label)
            if focus_content(app, kb):
                tab_walk(app, kb, f, label, presses=12, frame="Helikodu")

    # Search with query text and its completion.
    if want("query search"):
        kb.press("Control_L", "f", wait=0.6)
        kb.type("genre")
        kb.press("Shift_L", "colon", wait=0.8)
        f.audit(app, "query search completion")
        kb.press("Escape", wait=0.4)
        kb.type("ocean year>2000")
        time.sleep(1.5)
        f.audit(app, "query search")
        kb.press("Escape", wait=0.6)

    # Now Playing, with its analyzer, while something plays.
    if want("now playing") and act("win", "now-playing"):
        actions.activate("app", "play-pause")
        actions.activate("win", "now-playing")
        time.sleep(1.5)
        f.audit(app, "now playing")
        tab_walk(app, kb, f, "now playing", presses=16, frame="Helikodu")
        for _ in range(30):
            n = app.focused()
            if n is not None and n[3] == "Analyzer":
                kb.press("space", wait=1.0)
                f.audit(app, "analyzer")
                break
            kb.press("Tab", wait=0.2)
        actions.activate("win", "now-playing")
        actions.activate("app", "stop")
        time.sleep(1.0)

    # Menus and popovers.
    if want("main menu"):
        popover(app, kb, f, "main menu", lambda: kb.press("F10", wait=0.2) or True)
    for label, name in (("output switcher", "output-switcher"), ("sleep timer", "sleep-timer")):
        if want(label) and act("win", name):
            popover(app, kb, f, label, act("win", name))

    # Windows.
    windows = [
        ("settings search", "app", "preferences"),
        ("keyboard shortcuts", "app", "shortcuts"),
        ("supported formats", "app", "formats"),
        ("about", "app", "about"),
        ("organize files", "app", "organize-files"),
        ("find duplicates", "app", "find-duplicates"),
        ("convert", "app", "convert"),
        ("sync and export", "app", "sync-export"),
        ("cover manager", "app", "cover-manager"),
        ("verify files", "app", "verify-files"),
        ("missing files", "app", "missing-files"),
    ]
    for label, scope, name in windows:
        if not want(label):
            continue
        opener = act(scope, name)
        if not opener:
            f.step(False, f"{label}: the action {scope}.{name} exists")
            continue
        after = None
        if label == "settings search":
            def after(frame):
                for _ in range(40):
                    n = app.focused()
                    if n is not None and n[1] == R.ENTRY:
                        kb.type("volume")
                        time.sleep(1.0)
                        f.audit(app, "settings search")
                        kb.press("Escape", wait=0.5)
                        return
                    kb.press("Tab", wait=0.2)
                f.step(False, "the settings search entry is reached with Tab")
        audit_window(app, kb, f, label, opener, after=after)

    # The mini player takes the main window's place (both are "Helikodu").
    if want("mini player") and act("app", "mini-player"):
        def sidebar_shown():
            return any(n[6][-1] == ("filler", "Library") and n[5].contains(Atspi.StateType.SHOWING)
                       for n in app.nodes())
        actions.activate("app", "mini-player")
        swapped = wait_for(lambda: not sidebar_shown(), timeout=5.0)
        f.step(swapped, "the mini player takes the window's place")
        time.sleep(0.8)
        f.audit(app, "mini player")
        tab_walk(app, kb, f, "mini player", presses=16, frame="Helikodu")
        kb.press("Control_L", "Shift_L", "m", wait=1.5)
        back = wait_for(sidebar_shown, timeout=5.0)
        if not back:
            actions.activate("app", "mini-player")
        f.step(back, "Ctrl+Shift+M brings the full window back from the mini player")


TRAY_WATCHER_XML = """
<node><interface name="org.kde.StatusNotifierWatcher">
  <method name="RegisterStatusNotifierItem"><arg type="s" direction="in"/></method>
  <method name="RegisterStatusNotifierHost"><arg type="s" direction="in"/></method>
  <property name="RegisteredStatusNotifierItems" type="as" access="read"/>
  <property name="IsStatusNotifierHostRegistered" type="b" access="read"/>
  <property name="ProtocolVersion" type="i" access="read"/>
  <signal name="StatusNotifierItemRegistered"><arg type="s"/></signal>
</interface></node>
"""


def tray_watcher(out):
    """Serves a StatusNotifierWatcher with a host, as a desktop's panel does,
    and writes the bus name of each item that registers to `out`."""
    items = []
    node = Gio.DBusNodeInfo.new_for_xml(TRAY_WATCHER_XML)

    def method(conn, sender, path, iface, name, params, invocation):
        if name == "RegisterStatusNotifierItem":
            item = params.unpack()[0]
            items.append(item if item.startswith(":") else sender)
            with open(out, "a") as o:
                o.write(f"{items[-1]}\n")
        invocation.return_value(None)

    def get_property(conn, sender, path, iface, name):
        return {"RegisteredStatusNotifierItems": GLib.Variant("as", items),
                "IsStatusNotifierHostRegistered": GLib.Variant("b", True),
                "ProtocolVersion": GLib.Variant("i", 0)}[name]

    def acquired(conn, name):
        conn.register_object("/StatusNotifierWatcher", node.interfaces[0], method,
                             get_property, None)

    Gio.bus_own_name(Gio.BusType.SESSION, "org.kde.StatusNotifierWatcher",
                     Gio.BusNameOwnerFlags.NONE, acquired, None, None)
    GLib.MainLoop().run()


def audit_tray(f, registered):
    """The tray menu as the desktop's panel reads it: every item that is not
    a separator needs a label, and the item a title and a tooltip."""
    try:
        names = [n for n in Path(registered).read_text().split() if n]
    except OSError:
        names = []
    if not f.step(bool(names), "the tray icon registers with the desktop's tray"):
        return
    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    item = names[-1]
    service, _, path = item.partition("/")
    path = "/" + path if path else "/StatusNotifierItem"

    def prop(obj, iface, name):
        return bus.call_sync(service, obj, "org.freedesktop.DBus.Properties", "Get",
                             GLib.Variant("(ss)", (iface, name)), None,
                             Gio.DBusCallFlags.NONE, 3000, None).unpack()[0]

    try:
        title = prop(path, "org.kde.StatusNotifierItem", "Title")
        tooltip = prop(path, "org.kde.StatusNotifierItem", "ToolTip")
        menu = prop(path, "org.kde.StatusNotifierItem", "Menu")
        layout = bus.call_sync(service, menu, "com.canonical.dbusmenu", "GetLayout",
                               GLib.Variant("(iias)", (0, -1, [])), None,
                               Gio.DBusCallFlags.NONE, 3000, None).unpack()[1]
    except GLib.Error as error:
        f.step(False, f"the tray menu can be read: {error.message}")
        return
    if not title.strip():
        f.problem("tray", "tray", "the tray item has no title")
    if not (tooltip[2] or "").strip():
        f.problem("tray", "tray", "the tray item has no tooltip title")
    labels = []

    def visit(node):
        _, properties, children = node
        kind = properties.get("type", "standard")
        visible = properties.get("visible", True)
        if kind != "separator" and visible and node[0] != 0:
            text = properties.get("label", "").replace("_", "")
            labels.append(text)
            if not text.strip():
                f.problem("tray", "tray", "a tray menu item has no label")
        for child in children:
            visit(child)

    visit(layout)
    f.audited["tray menu"] = len(labels)
    f.step(bool(labels), f"the tray menu has labelled items: {', '.join(labels)}")


def main():
    if len(sys.argv) == 3 and sys.argv[1] == "--tray-watcher":
        tray_watcher(sys.argv[2])
        return
    headless.in_private_session(sys.argv)
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("--library")
    p.add_argument("--database")
    p.add_argument("--target-dir")
    p.add_argument("--no-build", action="store_true")
    p.add_argument("--dump", help="append the accessibility tree of each audited view here")
    p.add_argument("--skip-path", action="store_true", help="leave out the keyboard path")
    p.add_argument("--skip-sweep", action="store_true",
                   help="leave out the other windows and views")
    p.add_argument("--views", help="audit only these windows and views of the sweep "
                                   "(comma-separated labels, such as 'view Tracks,convert')")
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
            registered = profile.root / "tray-items"
            watcher = subprocess.Popen([sys.executable, __file__, "--tray-watcher",
                                        str(registered)])
            time.sleep(0.5)
            process = subprocess.Popen([str(target / "release" / "helikodu"), "--scheme", "dark"],
                                       env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            app = App(process)
            try:
                if not f.step(app.find(), "the application appears on the accessibility bus"):
                    sys.exit(1)
                time.sleep(2)
                kb = Keyboard()
                if not args.skip_path:
                    keyboard_path(app, kb, f, profile)
                if not args.skip_sweep:
                    views = set(v.strip() for v in args.views.split(",")) if args.views else None
                    sweep(app, kb, f, views)
                    if not views or "tray menu" in views:
                        audit_tray(f, registered)
            finally:
                watcher.terminate()
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
        stops = f.tab_stops.get(label)
        tabbed = f"; {stops} distinct Tab stops" if stops is not None else ""
        print(f"- {label}: {count} interactive objects{tabbed}")
    print("\n## Missing names or roles, focus problems\n")
    if not f.issues:
        print("None.")
    for (reg, problem, context), e in sorted(f.issues.items()):
        text = f"; content: {', '.join(repr(t) for t in e['text'])}" if e["text"] else ""
        print(f"- [{reg}] {problem} ×{e['count']} (in {context}; seen in {', '.join(sorted(e['views']))}){text}")
    failed = any(not ok for ok, _ in f.steps) or bool(f.issues)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
