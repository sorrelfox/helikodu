#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
# SPDX-License-Identifier: GPL-3.0-or-later

"""Builds the website into a folder.

    docs/site/build.py --out _site
    docs/site/build.py --out _site --serve      # then open http://127.0.0.1:8000/

The site is made of:

- the pages in `pages/`, put into `layout.html` (the start page, and the page
  shown for an address that does not exist);
- the user guide and the FAQ, from `docs/user/`, read by the same code that
  builds the page the application installs for Help (`docs/user/build-guide.py`);
- the release notes, from `CHANGELOG.md`;
- `assets/` (the style sheet and the pictures) and the application icon.

The version comes from `Cargo.toml` and the languages from `po/LINGUAS`, so a
release publishes a site that names itself and its translations correctly.

When the pages are written, every link and picture in them is checked: one
that leads to a page, a file or a heading the site does not have is an error.
Links to other websites are not followed. Needs only Python 3.

Pages may use these marks, each replaced when the site is built:

    {{version}}  {{released}}  {{languages}}  {{language-count}}
    {{release-url}}  {{repository}}  {{root}}
    {{cover NAME}}            a generated cover, as the application draws them
    {{cover-defs}}            the drawings the covers refer to, once per page
    {{shot NAME | ALT}}       a screenshot, light or dark as the visitor's desktop
    {{viewer ID | LABEL}}     a set of screenshots with a button for each:
    {{tab TITLE | NAME | ALT | CAPTION}}
    {{end}}
"""

import argparse
import functools
import html
import http.server
import importlib.util
import re
import shutil
import sys
from html.parser import HTMLParser
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
PAGES = HERE / "pages"
ASSETS = HERE / "assets"
ICON = REPO / "data/icons/hicolor/scalable/apps/io.github.sorrelfox.Helikodu.svg"
REPOSITORY = "https://github.com/sorrelfox/helikodu"
ADDRESS = "https://sorrelfox.github.io/helikodu/"
# The path of the site on its server. The page for addresses that do not exist
# is shown at any address, so its links start from here.
BASE = "/helikodu/"

# The languages of the interface by their own names, for the codes of po/LINGUAS.
LANGUAGES = {
    "de": ("de", "Deutsch"),
    "es": ("es", "Español"),
    "et": ("et", "Eesti"),
    "fr": ("fr", "Français"),
    "it": ("it", "Italiano"),
    "ja": ("ja", "日本語"),
    "ko": ("ko", "한국어"),
    "nl": ("nl", "Nederlands"),
    "pt_BR": ("pt-BR", "Português (Brasil)"),
    "ru": ("ru", "Русский"),
    "uk": ("uk", "Українська"),
    "zh_CN": ("zh-Hans", "简体中文"),
}

MONTHS = ["January", "February", "March", "April", "May", "June", "July", "August",
          "September", "October", "November", "December"]

# The generated covers of the application (crates/ui-gtk: cover_art.rs and
# the palettes of the design tokens), drawn here as SVG: ground, shape, detail.
PALETTES = [
    ("#1F3B4D", "#E9B44C", "#F4EDE0"), ("#2B2D42", "#EF8354", "#BFC0C0"),
    ("#0F4C4A", "#F2E8CF", "#E07A5F"), ("#3D2C3E", "#F6C28B", "#C97B84"),
    ("#14213D", "#FCA311", "#E5E5E5"), ("#2F3E46", "#CAD2C5", "#84A98C"),
    ("#472D30", "#FFE1A8", "#E26D5C"), ("#1B263B", "#A9D6E5", "#E0E1DD"),
    ("#31363F", "#76ABAE", "#EEEEEE"), ("#40342A", "#D9A066", "#EDE0C8"),
]
# The cover of each part of the start page: its pattern and its palette.
COVERS = {
    "formats": ("stripes", 4),
    "playback": ("disc", 6),
    "output": ("diagonal", 7),
    "sound": ("arch", 9),
    "library": ("horizon", 2),
    "browse": ("two-discs", 1),
    "search": ("disc", 0),
    "playlists": ("stripes", 3),
    "files": ("diagonal", 5),
    "desktop": ("horizon", 8),
    "cli": ("arch", 4),
    "offline": ("two-discs", 6),
}


def fail(message):
    sys.exit(f"build: {message}")


def version():
    cargo = (REPO / "Cargo.toml").read_text(encoding="utf-8")
    m = re.search(r'^\[workspace\.package\]\n(?:(?!\[).*\n)*?version = "([^"]+)"', cargo, re.M)
    if not m:
        fail("Cargo.toml has no [workspace.package] version")
    return m.group(1)


def long_date(iso):
    year, month, day = (int(part) for part in iso.split("-"))
    return f"{day} {MONTHS[month - 1]} {year}"


def languages():
    """The interface languages as HTML, English first, and how many they are."""
    codes = [line.strip() for line in (REPO / "po/LINGUAS").read_text(encoding="utf-8").splitlines()
             if line.strip() and not line.startswith("#")]
    for code in codes:
        if code not in LANGUAGES:
            print(f"build: no name for the language {code}; add it to LANGUAGES in build.py",
                  file=sys.stderr)
    names = sorted((LANGUAGES.get(code, (code.replace("_", "-"), code)) for code in codes),
                   key=lambda pair: pair[1].casefold())
    items = ["English"] + [f'<span lang="{tag}">{html.escape(name)}</span>' for tag, name in names]
    return ", ".join(items), len(items)


def webp_size(path):
    """The width and height of a WebP picture, from its header."""
    data = path.read_bytes()[:32]
    if data[:4] != b"RIFF" or data[8:12] != b"WEBP":
        fail(f"{path.name} is not a WebP picture")
    kind = data[12:16]
    if kind == b"VP8L":
        bits = int.from_bytes(data[21:25], "little")
        return (bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1
    if kind == b"VP8X":
        return (int.from_bytes(data[24:27], "little") + 1,
                int.from_bytes(data[27:30], "little") + 1)
    if kind == b"VP8 ":
        return (int.from_bytes(data[26:28], "little") & 0x3FFF,
                int.from_bytes(data[28:30], "little") & 0x3FFF)
    fail(f"{path.name}: unknown kind of WebP picture")


def cover_shapes(pattern, shape, detail):
    if pattern == "disc":
        return f'<circle cx="68" cy="34" r="21" fill="{shape}"/>'
    if pattern == "diagonal":
        return f'<path d="M100 0v100H0z" fill="{shape}"/>'
    if pattern == "arch":
        return (f'<circle cx="50" cy="100" r="56" fill="{shape}"/>'
                f'<circle cx="50" cy="100" r="38" fill="{detail}"/>')
    if pattern == "stripes":
        return "".join(f'<rect x="{7 + 20 * i}" width="6" height="100" fill="{shape}"/>'
                       for i in range(5))
    if pattern == "horizon":
        return (f'<rect y="62" width="100" height="4" fill="{detail}"/>'
                f'<rect y="66" width="100" height="34" fill="{shape}"/>')
    if pattern == "two-discs":
        return (f'<circle cx="30" cy="70" r="12" fill="{detail}"/>'
                f'<circle cx="62" cy="40" r="30" fill="{shape}"/>')
    fail(f"no cover pattern {pattern}")


def cover_defs():
    symbols = []
    for name, (pattern, palette) in COVERS.items():
        ground, shape, detail = PALETTES[palette]
        symbols.append(f'<symbol id="cover-{name}" viewBox="0 0 100 100">'
                       f'<rect width="100" height="100" fill="{ground}"/>'
                       f"{cover_shapes(pattern, shape, detail)}</symbol>")
    return ('<svg class="defs" width="0" height="0" aria-hidden="true" focusable="false">'
            f'{"".join(symbols)}</svg>')


def cover(name):
    if name not in COVERS:
        fail(f"no cover named {name}")
    return ('<svg class="cover" viewBox="0 0 100 100" aria-hidden="true" focusable="false">'
            f'<use href="#cover-{name}"/></svg>')


class Site:
    """What the pages are built from, and the pages written so far."""

    def __init__(self, out):
        self.out = out
        self.version = version()
        self.changelog = (REPO / "CHANGELOG.md").read_text(encoding="utf-8")
        m = re.search(rf"^## \[{re.escape(self.version)}\] - (\d{{4}}-\d\d-\d\d)$",
                      self.changelog, re.M)
        if not m:
            fail(f"CHANGELOG.md has no section for version {self.version}")
        self.released = long_date(m.group(1))
        self.languages, self.language_count = languages()
        self.layout = (HERE / "layout.html").read_text(encoding="utf-8")
        self.written = []

    def shot(self, name, alt, root, eager=False):
        """A screenshot that follows the visitor's colour scheme."""
        light = ASSETS / "shots" / f"{name}-light.webp"
        dark = ASSETS / "shots" / f"{name}-dark.webp"
        for path in (light, dark):
            if not path.is_file():
                fail(f"no picture {path.relative_to(HERE)}; run docs/site/shots.py")
        # The pictures hold two pixels for each logical pixel. A window can
        # differ by a pixel or two between the schemes, so each has its size.
        (width, height), (dark_width, dark_height) = webp_size(light), webp_size(dark)
        loading = 'fetchpriority="high"' if eager else 'loading="lazy"'
        return (f'<picture><source srcset="{root}assets/shots/{name}-dark.webp" '
                f'media="(prefers-color-scheme: dark)" '
                f'width="{dark_width // 2}" height="{dark_height // 2}">'
                f'<img src="{root}assets/shots/{name}-light.webp" alt="{html.escape(alt)}" '
                f'width="{width // 2}" height="{height // 2}" {loading} decoding="async">'
                f"</picture>")

    def viewers(self, text, root):
        """Replaces each {{viewer}} … {{end}} block by its buttons and figures."""
        def one(m):
            ident, label = (part.strip() for part in m.group(1).split("|", 1))
            buttons, figures = [], []
            for line in m.group(2).strip().splitlines():
                tab = re.fullmatch(r"\s*\{\{tab (.+)\}\}\s*", line)
                if not tab:
                    fail(f"viewer {ident}: only {{{{tab …}}}} lines may stand in it: {line.strip()}")
                parts = [part.strip() for part in tab.group(1).split("|")]
                if len(parts) != 4:
                    fail(f"viewer {ident}: a tab is TITLE | NAME | ALT | CAPTION: {line.strip()}")
                title, name, alt, caption = parts
                checked = " checked" if not buttons else ""
                buttons.append(f'<label><input type="radio" name="{ident}"{checked}>'
                               f"<span>{title}</span></label>")
                figures.append(f"<figure>{self.shot(name, alt, root)}"
                               f"<figcaption>{caption}</figcaption></figure>")
            if not buttons:
                fail(f"viewer {ident} has no tabs")
            return (f'<div class="viewer" id="{ident}"><fieldset class="seg">'
                    f'<legend class="visually-hidden">{label}</legend>{"".join(buttons)}'
                    f'</fieldset><div class="stage">{"".join(figures)}</div></div>')
        return re.sub(r"\{\{viewer ([^}]+)\}\}\n(.*?)\n\s*\{\{end\}\}", one, text, flags=re.S)

    def marks(self, text, root):
        """Replaces the marks of a page (see the top of this file)."""
        text = self.viewers(text, root)

        def shot(m):
            name, alt = (part.strip() for part in m.group(2).split("|", 1))
            return self.shot(name, alt, root, eager=m.group(1) == "hero-shot")

        text = re.sub(r"\{\{(shot|hero-shot) ([^}]+)\}\}", shot, text)
        text = re.sub(r"\{\{cover ([a-z-]+)\}\}", lambda m: cover(m.group(1)), text)
        simple = {
            "cover-defs": cover_defs(),
            "version": html.escape(self.version),
            "released": self.released,
            "languages": self.languages,
            "language-count": str(self.language_count),
            "release-url": f"{REPOSITORY}/releases/tag/v{self.version}",
            "repository": REPOSITORY,
            "root": root,
        }
        text = re.sub(r"\{\{([a-z-]+)\}\}",
                      lambda m: simple[m.group(1)] if m.group(1) in simple else m.group(0), text)
        left = re.search(r"\{\{[^}]*\}\}", text)
        if left:
            fail(f"unknown mark {left.group(0)}")
        return text

    def write(self, path, title, description, content, current="", body_class=""):
        """Puts `content` into the layout and writes it as `path` of the site."""
        root = BASE if path == "404.html" else "../" * path.count("/") or "./"
        page = self.layout
        nav = {"features": "", "get": "", "guide": "", "changelog": ""}
        if current in nav:
            nav[current] = ' aria-current="page"'
        for key, value in nav.items():
            page = page.replace(f"{{{{current-{key}}}}}", value)
        canonical = ADDRESS + re.sub(r"index\.html$", "", path)
        page = (page.replace("{{title}}", html.escape(title))
                .replace("{{description}}", html.escape(description))
                .replace("{{canonical}}", canonical)
                .replace("{{body-class}}", body_class)
                .replace("{{content}}", content))
        page = self.marks(page, root)
        target = self.out / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(page, encoding="utf-8")
        self.written.append(target)

    def page(self, source, path, current=""):
        """A page of `pages/`: a head of `key: value` lines, a blank line, HTML."""
        text = (PAGES / source).read_text(encoding="utf-8")
        text = re.sub(r"\A<!--.*?-->\n", "", text, flags=re.S)
        head, _, content = text.partition("\n\n")
        fields = dict(line.split(": ", 1) for line in head.splitlines())
        if set(fields) - {"title", "description", "class"} or "title" not in fields:
            fail(f"{source}: the head needs title and description (and may have class)")
        self.write(path, fields["title"], fields.get("description", ""), content,
                   current=current, body_class=fields.get("class", ""))


def guide_builder():
    """docs/user/build-guide.py as a module: it knows the guide's Markdown."""
    spec = importlib.util.spec_from_file_location("build_guide", REPO / "docs/user/build-guide.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def guide_page(site, builder):
    title, lead, chapters = builder.contents()
    toc = "".join(f'<li><a href="#{key}">{name}</a></li>' for key, name, _ in chapters)
    content = f"""<div class="doc">
<nav class="doc-nav" aria-labelledby="contents"><p id="contents">Contents</p><ol>{toc}</ol></nav>
<article class="prose">
<h1>{title}</h1>
<p class="doc-note">For Helikodu {{{{version}}}}.</p>
{lead}
{chr(10).join(text for _, _, text in chapters)}
<p class="doc-note">The manual pages <code>helikodu(1)</code>, <code>helikodu-cli(1)</code> and
<code>helikodu-scan-worker(1)</code> describe the programs' options.</p>
</article>
</div>"""
    site.write("guide/index.html", "Helikodu user guide",
               "How to install and use Helikodu: the library, playback, the audio output, "
               "playlists, tags, the tools, settings and the command line, and the FAQ.",
               content, current="guide", body_class="doc-page")


def changelog_page(site, builder):
    """The release notes: CHANGELOG.md, one section for each version."""
    class Notes(builder.Chapter):
        """A piece of the changelog in the form the guide's renderer reads."""

        def __init__(self, key, text):
            self.key, self.text, self.title, self.anchors = key, text, "", set()
            self.path = Path("CHANGELOG.md")

    class Links:
        """The changelog links to websites only."""
        errors = []

        @staticmethod
        def link(href, _chapter):
            return href if re.match(r"[a-z]+:", href) else None

    sections = re.split(r"^## \[([^\]]+)\](?: - (\d{4}-\d\d-\d\d))?\n", site.changelog, flags=re.M)
    releases = []
    for name, date, body in zip(sections[1::3], sections[2::3], sections[3::3]):
        body = body.strip()
        if not body:
            continue
        key = "unreleased" if name == "Unreleased" else "v" + name
        # One level up: the version is this page's second-level heading.
        body = re.sub(r"^#(#+ )", r"\1", body, flags=re.M)
        text = builder.render(Notes(key, body), Links)
        if name == "Unreleased":
            head = '<h2 id="unreleased">Not yet released</h2>'
        else:
            head = (f'<h2 id="{key}">{html.escape(name)} <span class="when">{long_date(date)}</span></h2>'
                    f'<p class="doc-note"><a href="{REPOSITORY}/releases/tag/v{html.escape(name)}">'
                    f"The files of {html.escape(name)} on GitHub</a></p>")
        releases.append((key, name, head + text))
    toc = "".join(f'<li><a href="#{key}">{"Not yet released" if name == "Unreleased" else html.escape(name)}</a></li>'
                  for key, name, _ in releases)
    content = f"""<div class="doc">
<nav class="doc-nav" aria-labelledby="contents"><p id="contents">Versions</p><ul>{toc}</ul></nav>
<article class="prose">
<h1>Release notes</h1>
<p class="doc-note">What changed in each version of Helikodu, the newest first. Versions follow <a href="https://semver.org/spec/v2.0.0.html">Semantic Versioning</a>.</p>
{chr(10).join(f'<section class="release">{text}</section>' for _, _, text in releases)}
</article>
</div>"""
    site.write("changelog/index.html", "Helikodu release notes",
               "What was added, changed and fixed in each version of Helikodu, "
               "and the known issues of each release.",
               content, current="changelog", body_class="doc-page")


class Preview(http.server.SimpleHTTPRequestHandler):
    """Serves the built folder as the site's server does: also under the
    site's path, and with 404.html for what does not exist."""

    def translate_path(self, path):
        if path.startswith(BASE):
            path = path[len(BASE) - 1:]
        return super().translate_path(path)

    def send_error(self, code, message=None, explain=None):
        page = Path(self.directory) / "404.html"
        if code != 404 or not page.is_file():
            super().send_error(code, message, explain)
            return
        body = page.read_bytes()
        self.send_response(404)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def log_message(self, format, *args):
        pass


class Refs(HTMLParser):
    """The ids of a page and what it links to."""

    def __init__(self):
        super().__init__()
        self.ids, self.links = set(), []

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if attrs.get("id"):
            self.ids.add(attrs["id"])
        for key in ("href", "src"):
            if attrs.get(key) is not None:
                self.links.append(attrs[key])
        for candidate in (attrs.get("srcset") or "").split(","):
            if candidate.strip():
                self.links.append(candidate.split()[0])


def check_links(site):
    """Every local link of the written pages leads to a file of the site,
    and every fragment to an id of its page. Every screenshot is shown
    somewhere."""
    pages = {}
    for path in site.written:
        refs = Refs()
        refs.feed(path.read_text(encoding="utf-8"))
        pages[path.resolve()] = refs
    errors = []
    shown = {link.rsplit("/", 1)[-1] for refs in pages.values() for link in refs.links}
    for picture in sorted((ASSETS / "shots").glob("*.webp")):
        if picture.name not in shown:
            errors.append(f"assets/shots/{picture.name} is on no page; remove it, or the "
                          "picture from crates/ui-gtk/examples/site_shots.rs")
    for path, refs in pages.items():
        path = Path(path)
        for link in refs.links:
            if re.match(r"[a-z][a-z0-9+.-]*:", link):
                continue
            target, _, fragment = link.partition("#")
            if target.startswith(BASE):
                file = (site.out / target[len(BASE):]).resolve()
            else:
                file = (path.parent / target).resolve() if target else path
            if file.is_dir():
                file = file / "index.html"
            where = path.relative_to(site.out.resolve())
            if file != site.out.resolve() and site.out.resolve() not in file.parents:
                errors.append(f"{where}: {link} leaves the site")
            elif not file.is_file():
                errors.append(f"{where}: {link}: no such file")
            elif fragment and fragment not in pages.get(file, Refs()).ids:
                errors.append(f"{where}: {link}: no such heading")
    if errors:
        fail("broken links:\n  " + "\n  ".join(errors))


def build(out):
    if out.exists():
        if out.is_dir() and any(out.iterdir()) and not (out / "assets" / "site.css").is_file():
            fail(f"{out} holds something else; choose an empty folder")
        shutil.rmtree(out)
    out.mkdir(parents=True)
    shutil.copytree(ASSETS, out / "assets")
    shutil.copyfile(ICON, out / "assets" / "helikodu.svg")

    site = Site(out)
    builder = guide_builder()
    site.page("index.html", "index.html")
    site.page("404.html", "404.html")
    guide_page(site, builder)
    changelog_page(site, builder)
    check_links(site)
    return site


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("--out", required=True, type=Path, help="the folder to write (it is replaced)")
    p.add_argument("--serve", nargs="?", const=8000, type=int, metavar="PORT",
                   help="then serve the folder on this computer (port 8000 without a number)")
    args = p.parse_args()
    site = build(args.out)
    print(f"build: {len(site.written)} pages for Helikodu {site.version} in {args.out}")
    if args.serve:
        handler = functools.partial(Preview, directory=str(args.out))
        with http.server.ThreadingHTTPServer(("127.0.0.1", args.serve), handler) as server:
            print(f"build: serving on http://127.0.0.1:{args.serve}/ (Ctrl+C ends it)")
            try:
                server.serve_forever()
            except KeyboardInterrupt:
                pass


if __name__ == "__main__":
    main()
