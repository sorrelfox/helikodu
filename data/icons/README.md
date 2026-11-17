# Application icons

The Helikodu application icon, installed into the `hicolor` theme under the application ID `org.helikodu.Helikodu`.

| File | Size | Use |
|---|---|---|
| `hicolor/scalable/apps/org.helikodu.Helikodu.svg` | 128 × 128 canvas, scalable | Full-colour icon for launchers, the dock, the window list, the about dialog and software centres |
| `hicolor/symbolic/apps/org.helikodu.Helikodu-symbolic.svg` | 16 × 16 | Single-colour icon for panels, the sound indicator and other places that recolour icons to match the text |

## Design

Helikodu means "home of sound" in Estonian (*heli*, sound; *kodu*, home). The mark draws the name: a roof chevron over three vertical level bars, so the sound lives under the roof.

- **Full-colour icon.** The mark sits on a rounded-square tile in the app's dark deck colour, with a darker base edge underneath and a faint top rim light for a little depth. The roof is in the default accent amber (`#F0B23C`, shaded from `#F8C55A` to `#EAA02C`), the bars in near-white. The dark tile keeps the amber and white mark readable on light and dark panels alike. The bars are 16 units wide and sit on the 8-unit grid, so they stay two whole pixels wide at 16 px.
- **Symbolic icon.** The same roof and bars on a 16 px grid, drawn as filled outlines only (no strokes), so GTK and KDE can recolour it. The bars are two pixels wide on whole pixels.

## Editing

Both files are hand-written SVG: no editor metadata, embedded images, external references or text. Keep them that way. After a change, render the icons at 16, 32, 64, 128 and 256 px on a light and a dark background and check that they stay crisp, for example:

```sh
rsvg-convert -w 32 -h 32 hicolor/scalable/apps/org.helikodu.Helikodu.svg -o /tmp/icon-32.png
```

The icons are licensed under CC-BY-SA-4.0.
