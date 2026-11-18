#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
# SPDX-License-Identifier: GPL-3.0-or-later

"""Renders the designed screens with GTK's default theme (Adwaita) and with
Breeze, in light and dark, and compares them with the design's reference
images.

    tests/ui/visual.py --design DIR [--out target/ui-visual]

DIR holds the reference images, one `<Screen>.png` per designed screen (dark;
`Main.light.png` for the light main window). The renders come from the
`screenshot`, `views` and `dialogs` examples of helikodu-ui-gtk over the
sample backend, under a headless compositor, with `GTK_THEME` set to
`Adwaita`, `Adwaita:dark`, `Breeze` and `Breeze-Dark` (Breeze is skipped when
it is not installed).

For each screen it writes `<theme>-<scheme>/<file>.png`, a difference image
`diff/<theme>-<scheme>-<Screen>.png` (the render, the reference and the
difference side by side) and `report.md` with:

- mean absolute difference (0-255) and the share of pixels that differ by more
  than 48 in any channel, against the reference (dark renders, and the light
  main window);
- a structural similarity (SSIM, grey, 8x8 blocks) against the reference;
- the 3x3 cell where the render differs most, to point at the region;
- how much the render changes between Adwaita and Breeze (the application
  draws its own look, so this should be close to zero).

Exits non-zero only when rendering fails; the comparison is for review, with
the tolerance `--max-mean` (default 12) flagging the screens to look at.
"""

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import headless  # noqa: E402

try:
    import numpy as np
    from PIL import Image
except ImportError:  # pragma: no cover
    print("visual: needs python3-pillow and python3-numpy; skipped", file=sys.stderr)
    sys.exit(0)

# Render file (without the scheme suffix) -> reference screen.
SCREENS = {
    # examples/screenshot.rs
    "main": "Main",
    "album": "Album",
    # examples/views.rs
    "tracks": "Tracks",
    "artists": "Artists",
    "genres": "Genres",
    "folders": "Folders",
    "playlist": "Playlist",
    "search": "Search",
    "no-results": "NoResults",
    "context-menu-popover": "ContextMenu",
    "main-menu-popover": "MainMenu",
    "play-menu-popover": "PlayMenu",
    "empty-library": "EmptyLibrary",
    # examples/dialogs.rs
    "settings-output": "Settings",
    "settings-library": "SettingsLibrary",
    "settings-playback": "SettingsPlayback",
    "settings-loudness": "SettingsLoudness",
    "settings-appearance": "SettingsAppearance",
    "settings-advanced": "SettingsAdvanced",
    "output-switcher": "OutputSwitcher",
    "tag-editor": "TagEditor",
    "tag-editor-cover": "TagEditorCover",
    "tag-editor-info": "TagEditorInfo",
    "formats": "Formats",
    "about": "About",
    "scanning": "Scanning",
    "volume-confirm": "VolumeConfirm",
    "first-run": "FirstRun",
    # examples/screenshot.rs --style classic (the reference is the top strip)
    "classic-main": "ClassicStyle",
}

THEMES = [
    ("adwaita", "light", "Adwaita"),
    ("adwaita", "dark", "Adwaita:dark"),
    ("breeze", "light", "Breeze"),
    ("breeze", "dark", "Breeze-Dark"),
]


def theme_installed(name):
    base = name.split(":")[0]
    if base == "Adwaita":
        return True  # built into GTK
    for root in ("/usr/share/themes", os.path.expanduser("~/.themes"),
                 os.path.expanduser("~/.local/share/themes")):
        if (Path(root) / base / "gtk-4.0").exists():
            return True
    return False


def normalise(name, scheme):
    """Strips the scheme from an example's file name; None when the file is of
    the other scheme."""
    stem = name[:-4]
    for dark_suffix, light_suffix in (("-dark", "-light"),):
        if f"{dark_suffix}" in stem or f"{light_suffix}" in stem:
            want, other = (dark_suffix, light_suffix) if scheme == "dark" else (light_suffix, dark_suffix)
            if other in stem:
                return None
            return stem.replace(want, "")
    # examples/dialogs.rs: no suffix is dark, ".light" is light.
    if stem.endswith(".light"):
        return stem[: -len(".light")] if scheme == "light" else None
    return stem if scheme == "dark" else None


def render(target, out, comp):
    """Runs the examples once per theme; returns {(theme, scheme): dir}."""
    dirs = {}
    for theme, scheme, gtk_theme in THEMES:
        if not theme_installed(gtk_theme):
            print(f"visual: GTK theme {gtk_theme} is not installed; skipped", file=sys.stderr)
            continue
        final = out / f"{theme}-{scheme}"
        shutil.rmtree(final, ignore_errors=True)
        final.mkdir(parents=True)
        for example, extra in (("screenshot", []), ("views", []), ("dialogs", []),
                               ("screenshot", ["--style", "classic"])):
            raw = out / "raw" / f"{theme}-{scheme}-{example}{'-'.join([''] + extra)}"
            shutil.rmtree(raw, ignore_errors=True)
            raw.mkdir(parents=True)
            env = comp.env(GTK_THEME=gtk_theme, GTK_A11Y="none", HELIKODU_GTK_TESTS="1")
            program = target / "release" / "examples" / example
            run = subprocess.run([str(program), "--out", str(raw), *extra], env=env,
                                 capture_output=True, text=True, timeout=300)
            if run.returncode != 0:
                print(f"visual: {example} {gtk_theme} failed:\n{run.stderr[-1500:]}", file=sys.stderr)
            for f in raw.glob("*.png"):
                name = normalise(f.name, scheme)
                if name is None:
                    continue
                if extra:
                    if name != "main":
                        continue
                    name = "classic-main"
                shutil.copyfile(f, final / f"{name}.png")
        dirs[(theme, scheme)] = final
    return dirs


def load(path, size=None):
    image = Image.open(path).convert("RGB")
    if size and image.size != size:
        image = image.resize(size, Image.LANCZOS)
    return np.asarray(image, dtype=np.float32)


def ssim(a, b):
    """Mean SSIM over 8x8 blocks of the grey images."""
    ga = a @ np.array([0.299, 0.587, 0.114], dtype=np.float32)
    gb = b @ np.array([0.299, 0.587, 0.114], dtype=np.float32)
    h, w = (ga.shape[0] // 8) * 8, (ga.shape[1] // 8) * 8
    ga = ga[:h, :w].reshape(h // 8, 8, w // 8, 8)
    gb = gb[:h, :w].reshape(h // 8, 8, w // 8, 8)
    ma, mb = ga.mean(axis=(1, 3)), gb.mean(axis=(1, 3))
    va, vb = ga.var(axis=(1, 3)), gb.var(axis=(1, 3))
    cov = ((ga - ma[:, None, :, None]) * (gb - mb[:, None, :, None])).mean(axis=(1, 3))
    c1, c2 = (0.01 * 255) ** 2, (0.03 * 255) ** 2
    s = ((2 * ma * mb + c1) * (2 * cov + c2)) / ((ma ** 2 + mb ** 2 + c1) * (va + vb + c2))
    return float(s.mean())


CELLS = [["top left", "top", "top right"], ["left", "middle", "right"],
         ["bottom left", "bottom", "bottom right"]]


def compare(render_path, reference_path, diff_path):
    ref_img = Image.open(reference_path)
    rend_img = Image.open(render_path)
    note = ""
    if rend_img.size != ref_img.size:
        note = f"size {rend_img.size[0]}x{rend_img.size[1]} vs {ref_img.size[0]}x{ref_img.size[1]}"
        if reference_path.stem == "ClassicStyle":
            rend_img = rend_img.crop((0, 0, rend_img.size[0], ref_img.size[1]))
            note = "top strip"
    size = ref_img.size
    a = np.asarray(rend_img.convert("RGB").resize(size, Image.LANCZOS)
                   if rend_img.size != size else rend_img.convert("RGB"), dtype=np.float32)
    b = load(reference_path)
    d = np.abs(a - b)
    mean = float(d.mean())
    over = float((d.max(axis=2) > 48).mean() * 100)
    s = ssim(a, b)
    h, w = d.shape[:2]
    worst, where = -1.0, ""
    for r in range(3):
        for c in range(3):
            cell = d[r * h // 3:(r + 1) * h // 3, c * w // 3:(c + 1) * w // 3].mean()
            if cell > worst:
                worst, where = float(cell), CELLS[r][c]
    heat = np.clip(d.max(axis=2) * 2, 0, 255).astype(np.uint8)
    strip = Image.new("RGB", (w * 3, h))
    strip.paste(Image.fromarray(a.astype(np.uint8)), (0, 0))
    strip.paste(Image.fromarray(b.astype(np.uint8)), (w, 0))
    strip.paste(Image.fromarray(heat).convert("RGB"), (2 * w, 0))
    strip.save(diff_path)
    return mean, over, s, where, worst, note


def theme_delta(p1, p2):
    a, b = Image.open(p1).convert("RGB"), Image.open(p2).convert("RGB")
    if a.size != b.size:
        return None
    d = np.abs(np.asarray(a, dtype=np.float32) - np.asarray(b, dtype=np.float32))
    return float(d.mean()), float((d.max(axis=2) > 48).mean() * 100)


def main():
    headless.in_private_session(sys.argv)
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("--design", required=True, help="folder of the reference <Screen>.png images")
    p.add_argument("--out", default=str(headless.REPO / "target" / "ui-visual"))
    p.add_argument("--target-dir", help="Cargo target folder")
    p.add_argument("--no-build", action="store_true")
    p.add_argument("--max-mean", type=float, default=12.0)
    args = p.parse_args()
    design = Path(args.design)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    target = Path(args.target_dir) if args.target_dir else headless.REPO / "target"
    if not args.no_build:
        headless.cargo_build(["-p", "helikodu-ui-gtk", "--example", "screenshot",
                              "--example", "views", "--example", "dialogs"], args.target_dir)
    with headless.Compositor(a11y=False) as comp:
        dirs = render(target, out, comp)
    (out / "diff").mkdir(exist_ok=True)
    lines = ["| Screen | Theme | Scheme | Mean diff | Pixels > 48 | SSIM | Most different | Note |",
             "|---|---|---|---|---|---|---|---|"]
    missing = []
    for name, screen in SCREENS.items():
        for (theme, scheme), d in sorted(dirs.items()):
            rendered = d / f"{name}.png"
            reference = design / (f"{screen}.light.png" if scheme == "light" else f"{screen}.png")
            if not rendered.exists():
                if (theme, scheme) == ("adwaita", "dark"):
                    missing.append(f"{screen} (not rendered)")
                continue
            if not reference.exists():
                continue
            mean, over, s, where, worst, note = compare(
                rendered, reference, out / "diff" / f"{theme}-{scheme}-{screen}.png")
            flag = " **look**" if mean > args.max_mean else ""
            lines.append(f"| {screen} | {theme} | {scheme} | {mean:.1f}{flag} | {over:.1f} % | "
                         f"{s:.3f} | {where} ({worst:.0f}) | {note} |")
    lines += ["", "Theme dependence (Adwaita vs Breeze, same scheme): mean diff / pixels > 48", "",
              "| Render | Light | Dark |", "|---|---|---|"]
    for name in SCREENS:
        cells = []
        for scheme in ("light", "dark"):
            a, b = dirs.get(("adwaita", scheme)), dirs.get(("breeze", scheme))
            if a and b and (a / f"{name}.png").exists() and (b / f"{name}.png").exists():
                delta = theme_delta(a / f"{name}.png", b / f"{name}.png")
                cells.append("size differs" if delta is None else f"{delta[0]:.2f} / {delta[1]:.2f} %")
            else:
                cells.append("n/a")
        lines.append(f"| {name} | {cells[0]} | {cells[1]} |")
    if missing:
        lines += ["", "Not rendered: " + ", ".join(missing)]
    report = "\n".join(lines) + "\n"
    (out / "report.md").write_text(report)
    print(report)
    print(f"renders and differences in {out}", file=sys.stderr)


if __name__ == "__main__":
    main()
