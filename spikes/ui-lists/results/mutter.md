## table 200k tracks, 8 columns
GTK 4.24.1, display GdkWaylandDisplay, renderer GskGLRenderer, scale 1 (surface 1.00), window 1440x900, stylesheet on
rows 200000, columns 8
model+view build 21.1 ms, first frame painted 97 ms after the window was presented
refresh interval reported by compositor: 16.67 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 117 | 16.95 | 16.67 | 16.67 | 50.00 | 1 | 0 | 0.00 | 0.01 | 0.01 | 0.02 | 150.7 |
| slow scroll | 490 | 16.30 | 16.67 | 16.67 | 16.67 | 0 | 0 | 1.45 | 2.38 | 2.71 | 2.97 | 151.0 |
| fast scroll | 359 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 2.91 | 3.58 | 4.05 | 4.85 | 151.1 |
| random jumps | 299 | 16.67 | 16.69 | 16.75 | 16.95 | 0 | 0 | 12.60 | 16.94 | 17.56 | 18.30 | 152.8 |
| theme switch every 0.5 s | 268 | 14.88 | 16.82 | 20.10 | 29.71 | 1 | 0 | 0.73 | 0.39 | 23.39 | 24.00 | 153.3 |
| idle | 119 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.01 | 0.01 | 0.01 | 0.05 | 153.3 |
RSS MiB: process start 24.4, after GTK init 45.2, after building views 53.1, end 153.3, peak 153.3
items materialised by the model: 65584, rows fetched (data built): 65583, wrapped to top: 0
cell widgets created: 1640, cell binds: 524664
stylesheet switch to end of next frame: mean 22.7 ms, p95 24.0, worst 24.0 (n=8)
theme log:
  [  0.000 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [  0.000 s] applied Dark, accent #3DAEE9 (startup)
  [  0.001 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  0.001 s] portal accent-color = #3DAEE9
  [ 21.059 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 21.059 s] applied Light, accent #3DAEE9 (scripted override)
  [ 21.561 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 21.561 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 22.064 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 22.064 s] applied Light, accent #3DAEE9 (scripted override)
  [ 22.577 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 22.577 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 23.082 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 23.082 s] applied Light, accent #3DAEE9 (scripted override)
  [ 23.586 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 23.586 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 24.094 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 24.094 s] applied Light, accent #3DAEE9 (scripted override)
  [ 24.596 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 24.596 s] applied Dark, accent #3DAEE9 (scripted override)
shell:
  gtk-decoration-layout = Some("icon:minimize,maximize,close")
  WindowControls start: empty=true, layout=None; end: empty=false, layout=None
  start buttons: []
  end buttons: ["image-button.minimize", "image-button.maximize", "image-button.close"]
  window css classes: ["background", "csd", "hk-dark", "hk"]
  decorated=true titlebar-hidden=true
  gtk-theme-name = Breeze, icon theme = Some("breeze-dark"), font = Some("Noto Sans  10")

## table 200k tracks, 12 columns
GTK 4.24.1, display GdkWaylandDisplay, renderer GskGLRenderer, scale 1 (surface 1.00), window 1440x900, stylesheet on
rows 200000, columns 12
model+view build 28.7 ms, first frame painted 119 ms after the window was presented
refresh interval reported by compositor: 16.67 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 117 | 16.95 | 16.67 | 16.67 | 50.00 | 1 | 0 | 0.00 | 0.00 | 0.01 | 0.01 | 155.8 |
| slow scroll | 480 | 16.64 | 16.68 | 16.78 | 17.12 | 0 | 0 | 1.76 | 3.10 | 3.41 | 3.51 | 156.2 |
| fast scroll | 359 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 3.67 | 4.69 | 5.52 | 7.42 | 156.3 |
| random jumps | 249 | 20.04 | 30.76 | 31.03 | 31.34 | 52 | 0 | 17.64 | 23.16 | 23.98 | 24.15 | 159.1 |
| theme switch every 0.5 s | 263 | 15.18 | 16.67 | 31.88 | 33.38 | 7 | 0 | 0.89 | 0.38 | 26.91 | 32.72 | 159.5 |
| idle | 119 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.00 | 0.01 | 0.01 | 0.04 | 159.5 |
RSS MiB: process start 24.3, after GTK init 45.1, after building views 55.8, end 159.5, peak 159.5
items materialised by the model: 55300, rows fetched (data built): 55299, wrapped to top: 0
cell widgets created: 2460, cell binds: 663588
stylesheet switch to end of next frame: mean 27.5 ms, p95 32.7, worst 32.7 (n=8)
theme log:
  [  0.000 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [  0.000 s] applied Dark, accent #3DAEE9 (startup)
  [  0.001 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  0.001 s] portal accent-color = #3DAEE9
  [ 21.092 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 21.092 s] applied Light, accent #3DAEE9 (scripted override)
  [ 21.605 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 21.605 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 22.110 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 22.110 s] applied Light, accent #3DAEE9 (scripted override)
  [ 22.626 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 22.626 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 23.144 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 23.144 s] applied Light, accent #3DAEE9 (scripted override)
  [ 23.642 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 23.642 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 24.153 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 24.153 s] applied Light, accent #3DAEE9 (scripted override)
  [ 24.668 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 24.668 s] applied Dark, accent #3DAEE9 (scripted override)
shell:
  gtk-decoration-layout = Some("icon:minimize,maximize,close")
  WindowControls start: empty=true, layout=None; end: empty=false, layout=None
  start buttons: []
  end buttons: ["image-button.minimize", "image-button.maximize", "image-button.close"]
  window css classes: ["background", "csd", "hk-dark", "hk"]
  decorated=true titlebar-hidden=true
  gtk-theme-name = Breeze, icon theme = Some("breeze-dark"), font = Some("Noto Sans  10")

## table 200k tracks, 8 columns, no stylesheet
GTK 4.24.1, display GdkWaylandDisplay, renderer GskGLRenderer, scale 1 (surface 1.00), window 1440x900, stylesheet off
rows 200000, columns 8
model+view build 21.6 ms, first frame painted 89 ms after the window was presented
refresh interval reported by compositor: 16.67 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 117 | 16.95 | 16.67 | 16.67 | 50.00 | 1 | 0 | 0.00 | 0.01 | 0.01 | 0.01 | 148.4 |
| slow scroll | 483 | 16.54 | 16.68 | 16.73 | 16.81 | 0 | 0 | 1.52 | 2.16 | 2.30 | 2.55 | 149.1 |
| fast scroll | 359 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 3.12 | 3.74 | 4.03 | 4.26 | 149.2 |
| random jumps | 299 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 11.45 | 14.67 | 14.95 | 15.64 | 151.0 |
| theme switch every 0.5 s | 245 | 16.26 | 16.69 | 16.72 | 16.98 | 0 | 0 | 0.61 | 0.31 | 18.22 | 18.64 | 151.4 |
| idle | 119 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.00 | 0.00 | 0.01 | 0.04 | 151.4 |
RSS MiB: process start 24.4, after GTK init 45.0, after building views 52.9, end 151.4, peak 151.4
items materialised by the model: 67536, rows fetched (data built): 67535, wrapped to top: 0
cell widgets created: 1640, cell binds: 540280
stylesheet switch to end of next frame: mean 17.3 ms, p95 18.6, worst 18.6 (n=8)
theme log:
  [  0.000 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [  0.000 s] applied Dark, accent #3DAEE9 (startup)
  [  0.001 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  0.001 s] portal accent-color = #3DAEE9
  [ 21.052 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 21.052 s] applied Light, accent #3DAEE9 (scripted override)
  [ 21.552 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 21.552 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 22.054 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 22.054 s] applied Light, accent #3DAEE9 (scripted override)
  [ 22.553 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 22.553 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 23.054 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 23.054 s] applied Light, accent #3DAEE9 (scripted override)
  [ 23.554 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 23.554 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 24.055 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 24.055 s] applied Light, accent #3DAEE9 (scripted override)
  [ 24.555 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 24.555 s] applied Dark, accent #3DAEE9 (scripted override)
shell:
  gtk-decoration-layout = Some("icon:minimize,maximize,close")
  WindowControls start: empty=true, layout=None; end: empty=false, layout=None
  start buttons: []
  end buttons: ["image-button.minimize", "image-button.maximize", "image-button.close"]
  window css classes: ["background", "csd", "hk-dark"]
  decorated=true titlebar-hidden=true
  gtk-theme-name = Breeze, icon theme = Some("breeze-dark"), font = Some("Noto Sans  10")

## table 100 tracks (memory baseline)
GTK 4.24.1, display GdkWaylandDisplay, renderer GskGLRenderer, scale 1 (surface 1.00), window 1440x900, stylesheet on
rows 100, columns 8
model+view build 14.1 ms, first frame painted 89 ms after the window was presented
refresh interval reported by compositor: 16.67 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 117 | 16.95 | 16.67 | 16.67 | 50.00 | 1 | 0 | 0.00 | 0.00 | 0.01 | 0.01 | 144.0 |
| slow scroll | 957 | 8.36 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.32 | 0.81 | 1.03 | 2.43 | 144.4 |
| fast scroll | 361 | 16.62 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.82 | 1.17 | 1.40 | 1.49 | 144.5 |
| random jumps | 299 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 1.01 | 1.48 | 1.60 | 1.81 | 144.5 |
| theme switch every 0.5 s | 246 | 16.26 | 16.67 | 16.67 | 17.00 | 0 | 0 | 0.54 | 0.32 | 15.36 | 17.44 | 144.6 |
| idle | 119 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.00 | 0.01 | 0.01 | 0.04 | 144.6 |
RSS MiB: process start 24.4, after GTK init 45.1, after building views 48.8, end 144.6, peak 144.6
items materialised by the model: 101, rows fetched (data built): 100, wrapped to top: 41
cell widgets created: 800, cell binds: 800
stylesheet switch to end of next frame: mean 15.2 ms, p95 17.4, worst 17.4 (n=8)
theme log:
  [  0.000 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [  0.000 s] applied Dark, accent #3DAEE9 (startup)
  [  0.001 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  0.001 s] portal accent-color = #3DAEE9
  [ 21.078 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 21.078 s] applied Light, accent #3DAEE9 (scripted override)
  [ 21.578 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 21.578 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 22.094 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 22.094 s] applied Light, accent #3DAEE9 (scripted override)
  [ 22.611 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 22.611 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 23.128 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 23.128 s] applied Light, accent #3DAEE9 (scripted override)
  [ 23.627 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 23.627 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 24.142 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 24.142 s] applied Light, accent #3DAEE9 (scripted override)
  [ 24.661 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 24.661 s] applied Dark, accent #3DAEE9 (scripted override)
shell:
  gtk-decoration-layout = Some("icon:minimize,maximize,close")
  WindowControls start: empty=true, layout=None; end: empty=false, layout=None
  start buttons: []
  end buttons: ["image-button.minimize", "image-button.maximize", "image-button.close"]
  window css classes: ["background", "csd", "hk-dark", "hk"]
  decorated=true titlebar-hidden=true
  gtk-theme-name = Breeze, icon theme = Some("breeze-dark"), font = Some("Noto Sans  10")

## grid 20k albums, max-columns 12
GTK 4.24.1, display GdkWaylandDisplay, renderer GskGLRenderer, scale 1 (surface 1.00), window 1440x900, stylesheet on
albums 20000, cover files 4000, workers 4, texture cache 600, grid max-columns 12
model+view build 12.9 ms, first frame painted 102 ms after the window was presented
refresh interval reported by compositor: 16.67 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 118 | 16.95 | 16.67 | 16.67 | 50.00 | 1 | 0 | 0.03 | 0.02 | 0.30 | 1.18 | 181.9 |
| slow scroll | 481 | 16.60 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.56 | 0.84 | 1.09 | 4.47 | 194.1 |
| fast scroll | 359 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 3.54 | 5.86 | 6.38 | 9.07 | 229.2 |
| after fast (covers fill) | 119 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.01 | 0.00 | 0.01 | 0.29 | 229.2 |
| random jumps | 287 | 17.41 | 19.04 | 29.33 | 30.78 | 4 | 0 | 16.75 | 19.17 | 20.97 | 27.42 | 226.5 |
| after jumps (covers fill) | 120 | 16.53 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.05 | 0.21 | 0.44 | 1.23 | 226.5 |
| theme switch every 0.5 s | 253 | 15.77 | 16.68 | 16.80 | 20.12 | 0 | 0 | 0.61 | 0.26 | 17.31 | 23.45 | 227.9 |
| idle | 119 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.02 | 0.03 | 0.03 | 0.08 | 227.9 |
- warm-up (idle): at end of phase: 0 visible tiles without cover, 102 bound tiles without cover
- slow scroll: at end of phase: 0 visible tiles without cover, 0 bound tiles without cover
- fast scroll: at end of phase: 0 visible tiles without cover, 0 bound tiles without cover
- after fast (covers fill): all visible covers present 0 ms after scrolling stopped
- after fast (covers fill): at end of phase: 0 visible tiles without cover, 0 bound tiles without cover
- random jumps: at end of phase: 16 visible tiles without cover, 340 bound tiles without cover
- after jumps (covers fill): all visible covers present 15 ms after scrolling stopped
- after jumps (covers fill): at end of phase: 0 visible tiles without cover, 125 bound tiles without cover
- theme switch every 0.5 s: at end of phase: 0 visible tiles without cover, 125 bound tiles without cover
- idle: all visible covers present 0 ms after scrolling stopped
- idle: at end of phase: 0 visible tiles without cover, 125 bound tiles without cover
RSS MiB: process start 24.5, after GTK init 45.2, after building views 63.4, end 227.9, peak 227.9
items materialised by the model: 128253, rows fetched (data built): 128252, wrapped to top: 0
tile widgets created: 385, most tiles bound at once: 385
covers: LoaderStats { requested: 108276, cancelled: 5847, dropped_oldest: 86606, requeued_visible: 1203, decoded: 17026 }, cache hits 19976, stale results 1215, textures cached 600
cover decode+scale per image (worker): mean 1.4 ms, p95 1.7, worst 3.2 (n=17026)
bind-to-cover latency: p50 15.3 ms, p95 20.1, p99 106.7, worst 8579.0 (n=15811)
stylesheet switch to end of next frame: mean 17.4 ms, p95 23.3, worst 23.3 (n=8)
theme log:
  [  0.000 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [  0.000 s] applied Dark, accent #3DAEE9 (startup)
  [  0.001 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  0.001 s] portal accent-color = #3DAEE9
  [ 25.095 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 25.095 s] applied Light, accent #3DAEE9 (scripted override)
  [ 25.600 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 25.600 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 26.099 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 26.099 s] applied Light, accent #3DAEE9 (scripted override)
  [ 26.617 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 26.617 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 27.134 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 27.134 s] applied Light, accent #3DAEE9 (scripted override)
  [ 27.635 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 27.635 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 28.135 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 28.135 s] applied Light, accent #3DAEE9 (scripted override)
  [ 28.635 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 28.635 s] applied Dark, accent #3DAEE9 (scripted override)
shell:
  gtk-decoration-layout = Some("icon:minimize,maximize,close")
  WindowControls start: empty=true, layout=None; end: empty=false, layout=None
  start buttons: []
  end buttons: ["image-button.minimize", "image-button.maximize", "image-button.close"]
  window css classes: ["background", "csd", "hk-dark", "hk"]
  decorated=true titlebar-hidden=true
  gtk-theme-name = Breeze, icon theme = Some("breeze-dark"), font = Some("Noto Sans  10")

## grid 20k albums, max-columns 20
GTK 4.24.1, display GdkWaylandDisplay, renderer GskGLRenderer, scale 1 (surface 1.00), window 1440x900, stylesheet on
albums 20000, cover files 4000, workers 4, texture cache 600, grid max-columns 20
model+view build 16.4 ms, first frame painted 124 ms after the window was presented
refresh interval reported by compositor: 16.67 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 116 | 17.24 | 16.67 | 16.67 | 83.33 | 1 | 1 | 0.07 | 0.06 | 0.53 | 1.84 | 185.5 |
| slow scroll | 482 | 16.57 | 16.67 | 16.68 | 16.68 | 0 | 0 | 0.68 | 0.97 | 1.25 | 3.70 | 201.0 |
| fast scroll | 359 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 4.41 | 6.79 | 8.17 | 10.10 | 256.6 |
| after fast (covers fill) | 119 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.01 | 0.01 | 0.01 | 0.39 | 256.6 |
| random jumps | 185 | 26.96 | 33.21 | 33.40 | 35.03 | 110 | 0 | 26.37 | 27.82 | 32.71 | 34.09 | 232.4 |
| after jumps (covers fill) | 120 | 16.53 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.07 | 0.26 | 0.63 | 1.06 | 232.4 |
| theme switch every 0.5 s | 241 | 16.55 | 16.70 | 29.51 | 33.50 | 5 | 0 | 0.89 | 0.30 | 24.28 | 34.18 | 233.1 |
| idle | 119 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.03 | 0.06 | 0.07 | 0.11 | 233.1 |
- warm-up (idle): at end of phase: 0 visible tiles without cover, 349 bound tiles without cover
- slow scroll: at end of phase: 3 visible tiles without cover, 225 bound tiles without cover
- fast scroll: at end of phase: 0 visible tiles without cover, 0 bound tiles without cover
- after fast (covers fill): all visible covers present 0 ms after scrolling stopped
- after fast (covers fill): at end of phase: 0 visible tiles without cover, 0 bound tiles without cover
- random jumps: at end of phase: 16 visible tiles without cover, 491 bound tiles without cover
- after jumps (covers fill): all visible covers present 19 ms after scrolling stopped
- after jumps (covers fill): at end of phase: 0 visible tiles without cover, 288 bound tiles without cover
- theme switch every 0.5 s: at end of phase: 0 visible tiles without cover, 288 bound tiles without cover
- idle: all visible covers present 0 ms after scrolling stopped
- idle: at end of phase: 0 visible tiles without cover, 288 bound tiles without cover
RSS MiB: process start 24.4, after GTK init 45.0, after building views 65.4, end 233.1, peak 253.1
items materialised by the model: 148094, rows fetched (data built): 148093, wrapped to top: 0
tile widgets created: 641, most tiles bound at once: 641
covers: LoaderStats { requested: 116573, cancelled: 1098, dropped_oldest: 100389, requeued_visible: 2121, decoded: 17207 }, cache hits 31520, stale results 788, textures cached 600
cover decode+scale per image (worker): mean 1.4 ms, p95 1.5, worst 3.8 (n=17207)
bind-to-cover latency: p50 24.7 ms, p95 35.6, p99 10476.4, worst 11520.3 (n=16419)
stylesheet switch to end of next frame: mean 24.3 ms, p95 34.1, worst 34.1 (n=8)
theme log:
  [  0.000 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [  0.000 s] applied Dark, accent #3DAEE9 (startup)
  [  0.001 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  0.001 s] portal accent-color = #3DAEE9
  [ 25.094 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 25.094 s] applied Light, accent #3DAEE9 (scripted override)
  [ 25.595 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 25.595 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 26.103 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 26.103 s] applied Light, accent #3DAEE9 (scripted override)
  [ 26.616 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 26.616 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 27.128 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 27.128 s] applied Light, accent #3DAEE9 (scripted override)
  [ 27.631 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 27.631 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 28.134 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 28.134 s] applied Light, accent #3DAEE9 (scripted override)
  [ 28.645 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 28.645 s] applied Dark, accent #3DAEE9 (scripted override)
shell:
  gtk-decoration-layout = Some("icon:minimize,maximize,close")
  WindowControls start: empty=true, layout=None; end: empty=false, layout=None
  start buttons: []
  end buttons: ["image-button.minimize", "image-button.maximize", "image-button.close"]
  window css classes: ["background", "csd", "hk-dark", "hk"]
  decorated=true titlebar-hidden=true
  gtk-theme-name = Breeze, icon theme = Some("breeze-dark"), font = Some("Noto Sans  10")

