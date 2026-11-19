#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
# SPDX-License-Identifier: GPL-3.0-or-later

"""Renders the screenshots of the website.

    docs/site/shots.py [--scheme dark|light] [--only NAME,NAME] [--keep-png]
    docs/site/shots.py --store [--only NAME,NAME]

Builds the `site_shots` example of helikodu-ui-gtk and runs it once for each
colour scheme under a headless compositor on a private D-Bus session, so
nothing touches the desktop it is started from. The example draws the real
windows over the sample library (invented albums, a player that only pretends
to play), at two pixels for each logical pixel. Each picture is then written
as `docs/site/assets/shots/<name>-<scheme>.webp`, lossless, and the dark
Albums view also becomes `assets/social.jpg`, the picture shown when a link
to the site is shared. The Home view is drawn once more for each language of
LANGUAGES below, as `home-<language>`, from the translations in `po/`.

With --store it renders the pictures for app stores such as Flathub instead,
the ones the metainfo lists: the windows at the same size and scale, drawn
with their frame (the shadow and the rounded corners), as PNG files in
`docs/site/assets/screenshots/` (STORE below says which, in the light scheme
and in the dark one).

The pictures use the font and the settings of the desktop the script runs on.
Run it before a release when the interface has changed, look at the result,
and commit the pictures that changed.

Needs cargo, dbus-run-session, msgfmt (gettext), Python 3 with Pillow, and
kwin_wayland (its virtual output can be scaled, so icons and covers are drawn
at full resolution) or mutter (sharp text, pictures at half resolution).
"""

import argparse
import os
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
SHOTS = HERE / "assets" / "shots"
SOCIAL = HERE / "assets" / "social.jpg"
INNER = "HELIKODU_SITE_SHOTS_INNER"
SCHEMES = {"dark": "Adwaita:dark", "light": "Adwaita"}
# The languages the Home view is also drawn in, each with the locale that
# gives it its dates and numbers where the computer has that locale.
LANGUAGES = {"de": "de_DE", "ru": "ru_RU", "ja": "ja_JP", "ko": "ko_KR"}
LOCALIZED = "home"
# The pictures for app stores, best first, in each scheme. The light ones come
# first: the stores want the default look, and some dark ones beside it.
STORE = {
    "light": ["albums", "album", "now-playing", "query", "smart-playlist",
              "settings-output", "eq-parametric", "tag-editor", "converter"],
    "dark": ["albums"],
}
STORE_DIR = HERE / "assets" / "screenshots"
# Pixels for each logical pixel.
SCALE = 2
# The logical size of the virtual output: room for the 1440 x 900 window.
OUTPUT = (1920, 1200)


def target_dir():
    return Path(os.environ.get("CARGO_TARGET_DIR", REPO / "target"))


def build():
    """Builds the example and returns its path."""
    cargo = os.environ.get("CARGO", "cargo")
    status = subprocess.call(
        [cargo, "build", "--locked", "-p", "helikodu-ui-gtk", "--example", "site_shots"],
        cwd=REPO)
    if status != 0:
        sys.exit("shots: the example did not build")
    return target_dir() / "debug" / "examples" / "site_shots"


def compositor(socket):
    """The command of a headless compositor listening on `socket`, and
    whether its output is scaled."""
    w, h = OUTPUT
    if shutil.which("kwin_wayland"):
        return ["kwin_wayland", "--virtual", "--no-lockscreen",
                "--width", str(w * SCALE), "--height", str(h * SCALE),
                "--scale", str(SCALE), "--socket", socket], True
    if shutil.which("mutter"):
        return ["mutter", "--headless", "--no-x11", "--virtual-monitor", f"{w}x{h}@60",
                f"--wayland-display={socket}"], False
    sys.exit("shots: neither kwin_wayland nor mutter is installed")


def inner(args):
    """Inside the private session: the compositor, then the example. What it
    has to say goes to the standard output; the session's standard error is
    the chatter of the D-Bus daemon and is not shown."""
    runtime = Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp"))
    socket = f"helikodu-site-shots-{os.getpid()}"
    command, scaled = compositor(socket)
    if not scaled:
        print("shots: mutter cannot scale its virtual output; covers and icons are "
              "drawn at half resolution", flush=True)
    server = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        for _ in range(200):
            if (runtime / socket).exists():
                break
            time.sleep(0.05)
        else:
            print("shots: the headless compositor did not start", flush=True)
            return 1
        time.sleep(0.5)
        env = dict(os.environ)
        env.pop("DISPLAY", None)
        env.update(
            WAYLAND_DISPLAY=socket,
            GDK_BACKEND="wayland",
            GTK_A11Y="none",
            GTK_THEME=SCHEMES[args.scheme],
            HELIKODU_GTK_TESTS="1",
            LC_ALL="C.UTF-8",
            LANGUAGE="en",
        )
        if args.store:
            # The window draws its frame, with the shadow and the rounded
            # corners, as GTK does on most desktops; under KWin it would not.
            env.update(GTK_CSD="1")
        if args.language:
            env.update(LANGUAGE=args.language, LC_ALL=locale_of(args.language),
                       HELIKODU_LOCALEDIR=str(catalogues()))
        command = [args.binary, "--out", args.raw, "--scheme", args.scheme,
                   "--scale", str(SCALE)]
        if args.store:
            command.append("--store")
        if args.only:
            command += ["--only", args.only]
        # GTK's warnings go to a file beside the pictures; the example's own
        # complaints are shown.
        log = Path(args.raw).with_suffix(".log")
        try:
            with open(log, "w", encoding="utf-8") as errors:
                status = subprocess.call(command, env=env, stderr=errors, timeout=900)
        except subprocess.TimeoutExpired:
            print("shots: the example did not finish in 15 minutes", flush=True)
            return 1
        for line in log.read_text(encoding="utf-8", errors="replace").splitlines():
            if line.startswith("site-shots:"):
                print(line, flush=True)
        return status
    finally:
        server.terminate()
        try:
            server.wait(5)
        except subprocess.TimeoutExpired:
            server.kill()


def catalogues():
    """The folder the translations are compiled into."""
    return target_dir() / "site-shots" / "locale"


def compile_catalogue(language):
    """Compiles po/<language>.po, so that the example finds it."""
    if shutil.which("msgfmt") is None:
        sys.exit("shots: msgfmt (gettext) is not installed")
    folder = catalogues() / language / "LC_MESSAGES"
    folder.mkdir(parents=True, exist_ok=True)
    status = subprocess.call(
        ["msgfmt", "-o", str(folder / "helikodu.mo"), str(REPO / "po" / f"{language}.po")])
    if status != 0:
        sys.exit(f"shots: po/{language}.po did not compile")


def locale_of(language):
    """A locale for `language`: its own where the computer has it, which also
    gives its dates and numbers, otherwise one gettext still translates in."""
    try:
        known = subprocess.run(["locale", "-a"], capture_output=True, text=True).stdout.split()
    except OSError:
        known = []
    name = LANGUAGES[language]
    if f"{name}.utf8" in known or f"{name}.UTF-8" in known:
        return f"{name}.UTF-8"
    return "en_US.UTF-8" if "en_US.utf8" in known or "en_US.UTF-8" in known else "C.UTF-8"


def render(scheme, binary, raw, only="", language="", store=False):
    """Runs the example for `scheme` in a private D-Bus session of its own
    process group, and ends whatever the session left running (the services
    D-Bus started for it)."""
    if shutil.which("dbus-run-session") is None:
        sys.exit("shots: dbus-run-session is not installed")
    env = dict(os.environ, **{INNER: "1"})
    env.pop("DBUS_SESSION_BUS_ADDRESS", None)
    command = ["dbus-run-session", "--", sys.executable, str(Path(__file__).resolve()),
               "--scheme", scheme, "--binary", str(binary), "--raw", str(raw)]
    if only:
        command += ["--only", only]
    if language:
        command += ["--language", language]
    if store:
        command.append("--store")
    session = subprocess.Popen(command, env=env, start_new_session=True,
                               stdin=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        return session.wait()
    finally:
        try:
            os.killpg(session.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass


def convert(raw, scheme):
    """Writes each PNG of `raw` as a lossless WebP file of the site."""
    from PIL import Image
    SHOTS.mkdir(parents=True, exist_ok=True)
    count = 0
    for png in sorted(raw.glob("*.png")):
        target = SHOTS / f"{png.stem}-{scheme}.webp"
        with Image.open(png) as image:
            image.save(target, "WEBP", lossless=True, quality=100, method=6)
        count += 1
    return count


def social(raw):
    """The picture for shared links: the dark Albums view, 1200 x 630."""
    from PIL import Image
    source = raw / "albums.png"
    if not source.exists():
        return
    with Image.open(source) as image:
        image = image.convert("RGB")
        width = 1200
        height = round(image.height * width / image.width)
        image = image.resize((width, height), Image.LANCZOS).crop((0, 0, width, 630))
        image.save(SOCIAL, "JPEG", quality=88, optimize=True, progressive=True)


def store(binary, only):
    """Renders the pictures for app stores as `docs/site/assets/screenshots/
    <name>.png`, and `<name>-dark.png` in the dark scheme. Returns the exit
    status."""
    from PIL import Image
    STORE_DIR.mkdir(parents=True, exist_ok=True)
    failed = False
    for scheme, names in STORE.items():
        if only:
            names = [name for name in names if name in only.split(",")]
        if not names:
            continue
        raw = target_dir() / "site-shots" / f"store-{scheme}"
        shutil.rmtree(raw, ignore_errors=True)
        raw.mkdir(parents=True)
        status = render(scheme, binary, raw, only=",".join(names), store=True)
        suffix = "" if scheme == "light" else f"-{scheme}"
        for name in names:
            png = raw / f"{name}.png"
            if not png.exists():
                print(f"shots: no {scheme} picture {name}", file=sys.stderr)
                failed = True
                continue
            with Image.open(png) as image:
                image.save(STORE_DIR / f"{name}{suffix}.png", "PNG", optimize=True)
        if status != 0:
            print(f"shots: the {scheme} pictures are incomplete (exit status {status})",
                  file=sys.stderr)
            failed = True
    print(f"shots: store pictures in {STORE_DIR.relative_to(REPO)}")
    return 1 if failed else 0


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("--scheme", choices=sorted(SCHEMES), help="render one scheme only")
    p.add_argument("--only", default="", help="render these pictures only, separated by commas")
    p.add_argument("--keep-png", action="store_true", help="keep the PNG files the example wrote")
    p.add_argument("--store", action="store_true",
                   help="render the pictures for app stores (Flathub) instead")
    p.add_argument("--binary", help=argparse.SUPPRESS)
    p.add_argument("--raw", help=argparse.SUPPRESS)
    p.add_argument("--language", default="", help=argparse.SUPPRESS)
    args = p.parse_args()

    if os.environ.get(INNER) == "1":
        sys.exit(inner(args))

    try:
        import PIL  # noqa: F401
    except ImportError:
        sys.exit("shots: needs Pillow (python3-pillow)")
    binary = build()
    if args.store:
        sys.exit(store(binary, args.only))
    wanted = [name for name in args.only.split(",") if name]
    languages = [language for language in LANGUAGES
                 if not wanted or f"{LOCALIZED}-{language}" in wanted]
    for language in languages:
        compile_catalogue(language)
    failed = False
    for scheme in [args.scheme] if args.scheme else sorted(SCHEMES):
        raw = target_dir() / "site-shots" / scheme
        shutil.rmtree(raw, ignore_errors=True)
        raw.mkdir(parents=True)
        status = render(scheme, binary, raw, only=args.only)
        for language in languages:
            # The same view in another language, under a name of its own.
            other = raw.with_name(f"{scheme}-{language}")
            shutil.rmtree(other, ignore_errors=True)
            other.mkdir(parents=True)
            status = render(scheme, binary, other, only=LOCALIZED, language=language) or status
            if (other / f"{LOCALIZED}.png").exists():
                shutil.move(other / f"{LOCALIZED}.png", raw / f"{LOCALIZED}-{language}.png")
            else:
                status = status or 1
            shutil.rmtree(other, ignore_errors=True)
            other.with_suffix(".log").unlink(missing_ok=True)
        if status != 0:
            print(f"shots: the {scheme} pictures are incomplete (exit status {status})",
                  file=sys.stderr)
            failed = True
        count = convert(raw, scheme)
        if scheme == "dark":
            social(raw)
        print(f"shots: {count} {scheme} pictures in {SHOTS.relative_to(REPO)}")
        if not args.keep_png:
            shutil.rmtree(raw, ignore_errors=True)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
