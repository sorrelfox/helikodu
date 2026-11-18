#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
# SPDX-License-Identifier: GPL-3.0-or-later

"""Builds the user guide as one HTML page, for the Help menu.

    docs/user/build-guide.py --out index.html [--version 0.1.0]

Reads docs/user/guide/README.md (the title, the introduction and the order of
the chapters), every chapter it lists, and docs/user/faq.md, and writes a
single page with its own styles and no scripts, fonts or network references:
it opens from the installed copy without a network, and through the desktop's
file-opening portal in a sandbox, where neighbouring files cannot be reached.

Links between chapters become links inside the page. A link to a chapter or a
heading that does not exist is an error, so the build also checks the guide.
Only the Markdown the guide uses is understood: headings, paragraphs, lists,
tables, fenced code, inline code, bold, italic and links. Needs only Python 3.
"""

import argparse
import html
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
GUIDE = HERE / "guide"
FAQ = HERE / "faq.md"


def slug(text):
    """A heading's anchor as GitHub makes it: lower case, punctuation removed,
    spaces turned into hyphens."""
    text = re.sub(r"`|\*\*|\*", "", text).strip().lower()
    text = re.sub(r"[^\w\- ]", "", text)
    return text.replace(" ", "-")


class Chapter:
    def __init__(self, key, path):
        self.key = key
        self.path = path
        self.text = path.read_text(encoding="utf-8")
        self.title = ""
        self.anchors = set()
        for line in self.lines():
            m = re.match(r"(#{1,4}) (.*)", line)
            if m:
                if len(m.group(1)) == 1 and not self.title:
                    self.title = m.group(2).strip()
                else:
                    self.anchors.add(slug(m.group(2)))
        if not self.title:
            sys.exit(f"build-guide: {path} has no title (# heading)")

    def lines(self):
        """The lines outside fenced code."""
        fenced = False
        for line in self.text.splitlines():
            if line.startswith("```"):
                fenced = not fenced
                continue
            if not fenced:
                yield line


class Book:
    def __init__(self):
        readme = (GUIDE / "README.md").read_text(encoding="utf-8")
        self.readme = readme
        self.chapters = []
        for target in re.findall(r"^\d+\. \[[^\]]+\]\(([^)#]+\.md)\)", readme, re.M):
            path = GUIDE / target
            if not path.is_file():
                sys.exit(f"build-guide: the contents list {target}, which does not exist")
            self.chapters.append(Chapter(path.stem, path))
        self.chapters.append(Chapter("faq", FAQ))
        listed = {c.path.name for c in self.chapters}
        for path in sorted(GUIDE.glob("*.md")):
            if path.name != "README.md" and path.name not in listed:
                sys.exit(f"build-guide: {path.name} is not in the contents of guide/README.md")
        self.by_key = {c.key: c for c in self.chapters}
        self.errors = []

    def link(self, href, chapter):
        """The target of a link written in `chapter`: inside the page for the
        guide's own files, unchanged for web addresses, None for other files
        of the repository (shown as text)."""
        if re.match(r"[a-z]+:", href):
            return href
        page, _, anchor = href.partition("#")
        if not page:
            key = chapter.key
        else:
            name = Path(page).name
            if not name.endswith(".md"):
                return None
            key = Path(name).stem
            if key == "README":
                key = "contents"
        if key == "contents":
            return "#contents"
        target = self.by_key.get(key)
        if target is None:
            self.errors.append(f"{chapter.path.name}: link to {href}: no such chapter")
            return "#contents"
        if not anchor:
            return f"#{key}"
        if anchor not in target.anchors:
            self.errors.append(f"{chapter.path.name}: link to {href}: no such heading")
        return f"#{key}--{anchor}"


def inline(text, book, chapter):
    """Inline Markdown: code, links, bold, italic. Everything else is escaped."""
    codes = []

    def keep_code(m):
        codes.append(f"<code>{html.escape(m.group(1))}</code>")
        return f"\x00{len(codes) - 1}\x00"

    text = re.sub(r"`([^`]+)`", keep_code, text)
    links = []

    def keep_link(m):
        label, href = m.group(1), m.group(2)
        target = book.link(href, chapter)
        inner = format_text(label)
        if target is None:
            links.append(inner)
        else:
            external = ' class="external"' if target.startswith(("http:", "https:")) else ""
            links.append(f'<a href="{html.escape(target)}"{external}>{inner}</a>')
        return f"\x01{len(links) - 1}\x01"

    text = re.sub(r"\[([^\]]+)\]\(([^)\s]+)\)", keep_link, text)
    text = format_text(text)
    text = re.sub(r"\x01(\d+)\x01", lambda m: links[int(m.group(1))], text)
    return re.sub(r"\x00(\d+)\x00", lambda m: codes[int(m.group(1))], text)


def format_text(text):
    text = html.escape(text, quote=False)
    text = re.sub(r"\*\*(.+?)\*\*", r"<strong>\1</strong>", text)
    text = re.sub(r"(?<![\w*])\*(?!\s)(.+?)(?<!\s)\*(?![\w*])", r"<em>\1</em>", text)
    return text


def split_row(line):
    line = line.strip()
    if line.startswith("|"):
        line = line[1:]
    if line.endswith("|") and not line.endswith("\\|"):
        line = line[:-1]
    cells = re.split(r"(?<!\\)\|", line)
    return [c.strip().replace("\\|", "|") for c in cells]


def render(chapter, book, skip_title=False):
    """The chapter's Markdown as HTML."""
    out = []
    lines = chapter.text.splitlines()
    i = 0
    seen_title = False

    def para(buf):
        if buf:
            out.append(f"<p>{inline(' '.join(s.strip() for s in buf), book, chapter)}</p>")
            buf.clear()

    buf = []
    while i < len(lines):
        line = lines[i]
        if line.startswith("```"):
            para(buf)
            code = []
            i += 1
            while i < len(lines) and not lines[i].startswith("```"):
                code.append(lines[i])
                i += 1
            if i == len(lines):
                sys.exit(f"build-guide: {chapter.path.name}: a code block is not closed")
            out.append(f"<pre><code>{html.escape(chr(10).join(code))}</code></pre>")
            i += 1
            continue
        m = re.match(r"(#{1,4}) (.*)", line)
        if m:
            para(buf)
            level, title = len(m.group(1)), m.group(2).strip()
            if level == 1 and not seen_title:
                seen_title = True
                if not skip_title:
                    out.append(f'<h2 id="{chapter.key}">{inline(title, book, chapter)}</h2>')
            else:
                anchor = f"{chapter.key}--{slug(title)}"
                tag = f"h{min(level + 1, 6)}"
                out.append(f'<{tag} id="{anchor}">{inline(title, book, chapter)}</{tag}>')
            i += 1
            continue
        if line.startswith("|"):
            para(buf)
            rows = []
            while i < len(lines) and lines[i].startswith("|"):
                rows.append(split_row(lines[i]))
                i += 1
            if len(rows) < 2 or not all(re.fullmatch(r":?-+:?", c) for c in rows[1]):
                sys.exit(f"build-guide: {chapter.path.name}: a table has no header line")
            head = "".join(f"<th>{inline(c, book, chapter)}</th>" for c in rows[0])
            body = "".join(
                "<tr>" + "".join(f"<td>{inline(c, book, chapter)}</td>" for c in r) + "</tr>"
                for r in rows[2:])
            out.append(f'<div class="table"><table><thead><tr>{head}</tr></thead>'
                       f"<tbody>{body}</tbody></table></div>")
            continue
        m = re.match(r"(- |\d+\. )", line)
        if m:
            para(buf)
            ordered = m.group(1)[0].isdigit()
            items = []
            while i < len(lines):
                item = re.match(r"(- |\d+\. )(.*)", lines[i])
                if item and (item.group(1)[0].isdigit()) == ordered:
                    items.append([item.group(2)])
                elif lines[i].startswith("  ") and lines[i].strip() and items:
                    items[-1].append(lines[i].strip())
                else:
                    break
                i += 1
            tag = "ol" if ordered else "ul"
            body = "".join(f"<li>{inline(' '.join(it), book, chapter)}</li>" for it in items)
            out.append(f"<{tag}>{body}</{tag}>")
            continue
        if line.startswith(">"):
            sys.exit(f"build-guide: {chapter.path.name}: block quotes are not supported")
        if not line.strip():
            para(buf)
        else:
            buf.append(line)
        i += 1
    para(buf)
    return "\n".join(out)


STYLE = """
:root { color-scheme: light dark; --bg: #f8f8f6; --fg: #1a1b1e; --muted: #474a51;
  --line: #d6d6d0; --code: #ececE9; --link: #6a4d15; }
@media (prefers-color-scheme: dark) {
  :root { --bg: #17181b; --fg: #edeef0; --muted: #b4b8c0; --line: #2a2c32; --code: #25272d;
    --link: #f3c169; }
}
html { background: var(--bg); color: var(--fg); }
body { max-width: 46rem; margin: 0 auto; padding: 1.5rem 1rem 4rem;
  font: 1rem/1.55 system-ui, sans-serif; }
h1 { font-size: 1.9rem; margin: 0.5rem 0 0.25rem; }
h2 { font-size: 1.45rem; margin: 3rem 0 0.5rem; padding-top: 1rem;
  border-top: 1px solid var(--line); }
h3 { font-size: 1.15rem; margin: 1.8rem 0 0.4rem; }
h4 { font-size: 1rem; margin: 1.4rem 0 0.3rem; }
p, li { color: var(--fg); }
a { color: var(--link); }
a:focus-visible { outline: 2px solid var(--link); outline-offset: 2px; }
code { font: 0.9em/1.4 ui-monospace, monospace; background: var(--code);
  padding: 0.05em 0.3em; border-radius: 4px; }
pre { background: var(--code); padding: 0.75rem 1rem; border-radius: 6px; overflow-x: auto; }
pre code { padding: 0; background: none; }
.table { overflow-x: auto; }
table { border-collapse: collapse; margin: 0.75rem 0; }
th, td { border: 1px solid var(--line); padding: 0.35rem 0.6rem; text-align: left;
  vertical-align: top; }
th { font-weight: 600; }
nav ol { padding-left: 1.5rem; }
.version, .note { color: var(--muted); }
@media (prefers-reduced-motion: no-preference) { html { scroll-behavior: smooth; } }
"""


def build(version):
    book = Book()
    intro = Chapter("contents", GUIDE / "README.md")
    # The introduction: the README without its numbered contents and the
    # closing "See also" line, which the page replaces.
    kept = []
    for line in intro.text.splitlines():
        if re.match(r"\d+\. \[", line) or line.startswith("See also"):
            continue
        kept.append(line)
    intro.text = "\n".join(kept)
    parts = [render(intro, book, skip_title=True)]
    toc = "".join(f'<li><a href="#{c.key}">{inline(c.title, book, c)}</a></li>'
                  for c in book.chapters)
    for chapter in book.chapters:
        parts.append(render(chapter, book))
    if book.errors:
        sys.exit("build-guide: broken links:\n  " + "\n  ".join(book.errors))
    version_line = (f'<p class="version">For Helikodu {html.escape(version)}.</p>'
                    if version else "")
    return f"""<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Helikodu user guide</title>
<style>{STYLE}</style>
</head>
<body>
<main>
<h1 id="top">{inline(intro.title, book, intro)}</h1>
{version_line}
{parts[0]}
<nav aria-labelledby="contents"><h2 id="contents">Contents</h2><ol>{toc}</ol></nav>
{chr(10).join(parts[1:])}
<p class="note">The manual pages <code>helikodu(1)</code>, <code>helikodu-cli(1)</code> and
<code>helikodu-scan-worker(1)</code> describe the programs' options.</p>
</main>
</body>
</html>
"""


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("--out", required=True, help="the page to write")
    p.add_argument("--version", default="", help="the version named on the page")
    args = p.parse_args()
    page = build(args.version)
    Path(args.out).write_text(page, encoding="utf-8")


if __name__ == "__main__":
    main()
