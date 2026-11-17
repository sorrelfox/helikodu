# Website

The static website, published with GitHub Pages at <https://sorrelfox.github.io/helikodu/>. It will later carry the user guide and the release notes.

## Content

- `index.html` – the single page: the name, a short description, the goals, the project status and links.
- `style.css` – the styles, with light and dark colours chosen by `prefers-color-scheme`.

The site uses no JavaScript, no external fonts, no CDNs and no trackers. The app icon is not copied here: the publishing workflow adds `data/icons/hicolor/scalable/apps/io.github.sorrelfox.Helikodu.svg` next to `index.html`. To preview locally, copy the icon into a temporary folder together with these files and open `index.html`.

## Publishing

`.github/workflows/pages.yml` runs on every push to `main` that changes `docs/site/` or the app icon, and can be started by hand. It assembles the folder `_site` from this folder plus the icon and deploys it with GitHub Pages. In the repository settings, Pages must use "GitHub Actions" as its source.

HTML and CSS are licensed under CC-BY-SA-4.0.
