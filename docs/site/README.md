# Website

The website of Helikodu, published with GitHub Pages at <https://sorrelfox.github.io/helikodu/>. It says what Helikodu is, shows what it does with screenshots, and carries the user guide and the release notes.

The site is plain HTML and CSS. It uses no JavaScript, no web fonts, no CDN and no tracker, and loads nothing from another server.

## Content

| File | What it is |
|---|---|
| `pages/index.html` | The start page: the name, a part for each area of what Helikodu does, how to get it and how to take part |
| `pages/404.html` | The page shown for an address that does not exist |
| `layout.html` | The frame of every page: the head, the header with the links, the footer |
| `assets/site.css` | The styles. The colours are the application's own, light and dark as the visitor's desktop asks |
| `assets/shots/` | The screenshots, each in the light and in the dark scheme, as lossless WebP at two pixels for each logical pixel |
| `assets/screenshots/` | The pictures for app stores such as Flathub, as PNG (see [Pictures for app stores](#pictures-for-app-stores)) |
| `assets/social.jpg` | The picture shown when a link to the site is shared |
| `build.py` | Builds the site |
| `shots.py` | Renders the screenshots |

Two pages have no file here: the user guide is built from `docs/user/` by the code that builds the page the application installs for Help (`docs/user/build-guide.py`), and the release notes are built from `CHANGELOG.md`. The application icon comes from `data/icons/`.

The version on the pages comes from `Cargo.toml` and the interface languages from `po/LINGUAS`, so the site names itself and its translations correctly when a release publishes it.

## Building and previewing

```sh
docs/site/build.py --out target/site --serve
```

This writes the site into `target/site` and serves it at <http://127.0.0.1:8000/> until you press Ctrl+C. Without `--serve` it only writes the folder. It needs nothing but Python 3.

The build checks the site: a link or picture that leads to a page, a file or a heading the site does not have stops it, and so does a screenshot that no page shows.

## Writing pages

A page of `pages/` starts with `title:` and `description:` lines (and `class:` for the class of its `body`), then a blank line, then HTML. Marks in double braces are replaced when the site is built; the top of `build.py` lists them. The ones used most:

- `{{version}}`, `{{released}}`, `{{languages}}`: the version, its date, the interface languages.
- `{{shot NAME | ALT}}`: the screenshot `assets/shots/NAME-light.webp`, or its `-dark` twin on a dark desktop, with its alternative text.
- `{{viewer ID | LABEL}}`, then one `{{tab TITLE | NAME | ALT | CAPTION}}` line for each picture, then `{{end}}`: several screenshots with a button for each.
- `{{cover NAME}}`: a generated cover as the application draws them for albums without one. `build.py` holds the pattern and the palette of each part's cover.

Write the text as the user guide is written: plainly, and only what the current version does. When a release changes what Helikodu can do, change the start page with it, and check the sentence about the state of the release under "Get Helikodu".

## Screenshots

```sh
docs/site/shots.py                      # every picture, both schemes
docs/site/shots.py --only albums,home   # some of them
```

The pictures are not photographs of a desktop. The example `site_shots` of `helikodu-ui-gtk` draws the real windows over the sample library (invented albums, artists and tracks, and a player that only pretends to play), and `shots.py` runs it for each colour scheme under a headless compositor on a private D-Bus session, then writes the pictures into `assets/shots/`. The Home view is drawn once more in each language listed at the top of `shots.py`, from the translations in `po/`, as `home-<language>`. The script needs cargo, `dbus-run-session`, `msgfmt` (gettext), Python 3 with Pillow, and `kwin_wayland` or `mutter`.

### Pictures for app stores

```sh
docs/site/shots.py --store
```

The same example draws the pictures that the metainfo (`data/io.github.sorrelfox.Helikodu.metainfo.xml`) lists for Flathub and other app stores. It writes them as PNG files into `assets/screenshots/`: `<name>.png` in the light scheme and `<name>-dark.png` in the dark one. `STORE` at the top of `shots.py` names them. They follow Flathub's quality guidelines, except for the size: the guidelines ask for windows of at most 1000 × 700, where Helikodu's three panes are cramped, and the pictures show the window at its usual size instead.

- each window has the size of the website's pictures (the main window 1440 × 900 logical pixels) at two pixels for each logical pixel, drawn with its own shadow and rounded corners and not maximized;
- the main window has the default, integrated style, with the window buttons in its header;
- the light pictures come first, and one dark picture shows the dark style.

Like the website's pictures, they use the font and the icon theme of the desktop they are made on.

The metainfo points at them on the `main` branch, with a caption each; the captions are translated with the rest of the metainfo. When `STORE` changes, change the `<screenshots>` list of the metainfo with it.

A new picture is a step in `crates/ui-gtk/examples/site_shots.rs` and a `{{shot}}` or `{{tab}}` mark on a page. Run `shots.py` before a release when the interface has changed, look at the pictures, and commit those that changed. They use the font and the desktop settings of the computer they are made on.

## Publishing

`.github/workflows/pages.yml` runs once a release has been published from a pushed tag, and can be started by hand from the Actions tab (for the latest release). It runs `build.py`, adds Helikodu's Flatpak repository in `repo/`, made from the release's Flatpak bundles ([docs/dev/flatpak.md](../dev/flatpak.md#the-flatpak-repository)), and deploys the result with GitHub Pages. `build.py` also writes `helikodu.flatpakref` and `helikodu.flatpakrepo`, which add that repository, from its public key `packaging/flatpak/helikodu-repo.gpg`. In the repository settings, Pages must use "GitHub Actions" as its source.

The pages, the styles and the pictures are licensed under CC-BY-SA-4.0, the two scripts under GPL-3.0-or-later.
