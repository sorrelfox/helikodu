## table 200k tracks, 8 columns
GTK 4.24.1, display GdkWaylandDisplay, renderer GskGLRenderer, scale 1 (surface 1.00), window 1440x900, stylesheet on
rows 200000, columns 8
model+view build 32.6 ms, first frame painted 112 ms after the window was presented
refresh interval reported by compositor: 13.34 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 146 | 13.61 | 13.34 | 16.67 | 50.00 | 1 | 0 | 0.00 | 0.00 | 0.01 | 0.01 | 154.2 |
| slow scroll | 601 | 13.31 | 13.34 | 13.34 | 13.36 | 0 | 0 | 1.98 | 2.67 | 2.99 | 3.51 | 154.4 |
| fast scroll | 449 | 13.34 | 13.34 | 13.34 | 13.35 | 0 | 0 | 2.89 | 4.19 | 4.58 | 5.67 | 154.5 |
| random jumps | 258 | 19.28 | 26.67 | 26.67 | 26.67 | 115 | 0 | 15.53 | 22.71 | 23.35 | 24.41 | 156.1 |
| theme switch every 0.5 s | 297 | 13.43 | 13.34 | 26.67 | 39.99 | 8 | 0 | 0.69 | 0.33 | 22.78 | 26.49 | 156.6 |
| idle | 149 | 13.34 | 13.34 | 13.34 | 13.34 | 0 | 0 | 0.00 | 0.00 | 0.00 | 0.04 | 156.6 |
RSS MiB: process start 24.2, after GTK init 43.2, after building views 56.8, end 156.6, peak 156.6
items materialised by the model: 58676, rows fetched (data built): 58675, wrapped to top: 0
cell widgets created: 1640, cell binds: 469400
stylesheet switch to end of next frame: mean 23.4 ms, p95 26.5, worst 26.5 (n=8)
theme log:
  [  0.000 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [  0.000 s] applied Dark, accent #3DAEE9 (startup)
  [  0.001 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  0.001 s] portal accent-color = #3DAEE9
  [ 21.092 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 21.092 s] applied Light, accent #3DAEE9 (scripted override)
  [ 21.599 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 21.599 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 22.109 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 22.109 s] applied Light, accent #3DAEE9 (scripted override)
  [ 22.615 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 22.615 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 23.121 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 23.121 s] applied Light, accent #3DAEE9 (scripted override)
  [ 23.627 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 23.627 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 24.133 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 24.133 s] applied Light, accent #3DAEE9 (scripted override)
  [ 24.640 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 24.640 s] applied Dark, accent #3DAEE9 (scripted override)
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
model+view build 43.8 ms, first frame painted 111 ms after the window was presented
refresh interval reported by compositor: 13.34 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 145 | 13.73 | 13.34 | 16.67 | 66.67 | 1 | 1 | 0.00 | 0.00 | 0.01 | 0.02 | 161.9 |
| slow scroll | 600 | 13.33 | 13.34 | 13.34 | 13.35 | 0 | 0 | 2.61 | 3.69 | 4.18 | 4.68 | 162.1 |
| fast scroll | 449 | 13.34 | 13.34 | 13.34 | 13.34 | 0 | 0 | 3.63 | 5.23 | 6.12 | 6.37 | 162.2 |
| random jumps | 201 | 24.76 | 39.75 | 39.85 | 39.96 | 137 | 0 | 22.13 | 30.95 | 31.63 | 32.16 | 165.0 |
| theme switch every 0.5 s | 313 | 12.74 | 13.34 | 27.87 | 39.82 | 8 | 0 | 0.79 | 0.26 | 28.73 | 33.74 | 165.4 |
| idle | 149 | 13.34 | 13.34 | 13.34 | 13.34 | 0 | 0 | 0.00 | 0.00 | 0.01 | 0.05 | 165.4 |
RSS MiB: process start 24.5, after GTK init 43.4, after building views 61.9, end 165.4, peak 165.4
items materialised by the model: 46997, rows fetched (data built): 46996, wrapped to top: 0
cell widgets created: 2460, cell binds: 563952
stylesheet switch to end of next frame: mean 29.1 ms, p95 33.7, worst 33.7 (n=8)
theme log:
  [  0.000 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [  0.000 s] applied Dark, accent #3DAEE9 (startup)
  [  0.001 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  0.001 s] portal accent-color = #3DAEE9
  [ 21.079 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 21.079 s] applied Light, accent #3DAEE9 (scripted override)
  [ 21.589 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 21.589 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 22.097 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 22.097 s] applied Light, accent #3DAEE9 (scripted override)
  [ 22.603 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 22.603 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 23.109 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 23.109 s] applied Light, accent #3DAEE9 (scripted override)
  [ 23.614 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 23.614 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 24.122 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 24.122 s] applied Light, accent #3DAEE9 (scripted override)
  [ 24.628 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 24.628 s] applied Dark, accent #3DAEE9 (scripted override)
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
model+view build 31.6 ms, first frame painted 105 ms after the window was presented
refresh interval reported by compositor: 13.34 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 146 | 13.61 | 13.34 | 16.67 | 50.00 | 1 | 0 | 0.00 | 0.00 | 0.01 | 0.01 | 152.4 |
| slow scroll | 600 | 13.33 | 13.34 | 13.34 | 13.34 | 0 | 0 | 1.42 | 1.99 | 2.18 | 2.40 | 152.9 |
| fast scroll | 449 | 13.34 | 13.34 | 13.34 | 13.34 | 0 | 0 | 3.00 | 3.58 | 3.72 | 4.00 | 153.0 |
| random jumps | 355 | 14.05 | 26.23 | 26.49 | 26.63 | 19 | 0 | 11.78 | 15.14 | 15.42 | 15.54 | 154.8 |
| theme switch every 0.5 s | 311 | 12.85 | 13.34 | 13.34 | 14.88 | 0 | 0 | 0.47 | 0.26 | 17.53 | 18.19 | 155.2 |
| idle | 149 | 13.34 | 13.34 | 13.34 | 13.34 | 0 | 0 | 0.00 | 0.01 | 0.02 | 0.04 | 155.2 |
RSS MiB: process start 24.2, after GTK init 43.2, after building views 56.6, end 155.2, peak 155.2
items materialised by the model: 78740, rows fetched (data built): 78739, wrapped to top: 0
cell widgets created: 1640, cell binds: 629912
stylesheet switch to end of next frame: mean 16.8 ms, p95 18.2, worst 18.2 (n=8)
theme log:
  [  0.000 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [  0.000 s] applied Dark, accent #3DAEE9 (startup)
  [  0.001 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  0.001 s] portal accent-color = #3DAEE9
  [ 21.084 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 21.084 s] applied Light, accent #3DAEE9 (scripted override)
  [ 21.593 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 21.593 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 22.101 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 22.101 s] applied Light, accent #3DAEE9 (scripted override)
  [ 22.608 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 22.608 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 23.115 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 23.115 s] applied Light, accent #3DAEE9 (scripted override)
  [ 23.620 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 23.620 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 24.125 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 24.125 s] applied Light, accent #3DAEE9 (scripted override)
  [ 24.632 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 24.632 s] applied Dark, accent #3DAEE9 (scripted override)
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
model+view build 17.7 ms, first frame painted 105 ms after the window was presented
refresh interval reported by compositor: 13.34 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 145 | 13.73 | 13.34 | 16.67 | 66.67 | 1 | 1 | 0.00 | 0.01 | 0.01 | 0.01 | 145.1 |
| slow scroll | 599 | 13.33 | 13.34 | 13.34 | 13.35 | 0 | 0 | 0.52 | 0.76 | 1.28 | 2.55 | 145.3 |
| fast scroll | 449 | 13.34 | 13.34 | 13.34 | 13.35 | 0 | 0 | 0.81 | 1.25 | 1.51 | 1.81 | 145.4 |
| random jumps | 374 | 13.34 | 13.34 | 13.34 | 13.34 | 0 | 0 | 1.05 | 1.52 | 1.84 | 2.20 | 145.4 |
| theme switch every 0.5 s | 335 | 11.91 | 13.34 | 26.49 | 26.65 | 5 | 0 | 0.44 | 0.26 | 16.60 | 17.76 | 145.6 |
| idle | 149 | 13.34 | 13.34 | 13.34 | 13.34 | 0 | 0 | 0.00 | 0.01 | 0.01 | 0.04 | 145.6 |
RSS MiB: process start 24.0, after GTK init 43.1, after building views 50.1, end 145.6, peak 145.6
items materialised by the model: 101, rows fetched (data built): 100, wrapped to top: 42
cell widgets created: 800, cell binds: 800
stylesheet switch to end of next frame: mean 16.6 ms, p95 17.8, worst 17.8 (n=8)
theme log:
  [  0.000 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [  0.000 s] applied Dark, accent #3DAEE9 (startup)
  [  0.001 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  0.001 s] portal accent-color = #3DAEE9
  [ 21.061 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 21.061 s] applied Light, accent #3DAEE9 (scripted override)
  [ 21.570 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 21.570 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 22.075 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 22.075 s] applied Light, accent #3DAEE9 (scripted override)
  [ 22.584 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 22.584 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 23.090 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 23.090 s] applied Light, accent #3DAEE9 (scripted override)
  [ 23.598 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 23.598 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 24.103 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 24.103 s] applied Light, accent #3DAEE9 (scripted override)
  [ 24.609 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 24.609 s] applied Dark, accent #3DAEE9 (scripted override)
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
model+view build 13.3 ms, first frame painted 110 ms after the window was presented
refresh interval reported by compositor: 13.34 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 145 | 13.72 | 13.34 | 16.67 | 66.67 | 1 | 1 | 0.03 | 0.02 | 0.68 | 1.15 | 182.8 |
| slow scroll | 600 | 13.33 | 13.34 | 13.34 | 13.34 | 0 | 0 | 0.55 | 0.87 | 1.10 | 4.23 | 196.0 |
| fast scroll | 449 | 13.34 | 13.34 | 13.34 | 13.34 | 0 | 0 | 3.30 | 4.17 | 5.43 | 6.00 | 230.5 |
| after fast (covers fill) | 149 | 13.34 | 13.34 | 13.34 | 13.34 | 0 | 0 | 0.00 | 0.01 | 0.01 | 0.05 | 230.5 |
| random jumps | 248 | 20.06 | 26.43 | 26.54 | 26.68 | 125 | 0 | 16.50 | 17.52 | 20.08 | 20.90 | 228.5 |
| after jumps (covers fill) | 150 | 13.25 | 13.34 | 13.34 | 13.34 | 0 | 0 | 0.03 | 0.03 | 0.40 | 0.82 | 228.5 |
| theme switch every 0.5 s | 304 | 13.12 | 13.34 | 26.55 | 26.99 | 5 | 0 | 0.50 | 0.25 | 15.74 | 27.45 | 229.1 |
| idle | 149 | 13.34 | 13.34 | 13.34 | 13.34 | 0 | 0 | 0.02 | 0.02 | 0.03 | 0.07 | 229.1 |
- warm-up (idle): at end of phase: 0 visible tiles without cover, 97 bound tiles without cover
- slow scroll: at end of phase: 0 visible tiles without cover, 0 bound tiles without cover
- fast scroll: at end of phase: 0 visible tiles without cover, 0 bound tiles without cover
- after fast (covers fill): all visible covers present 0 ms after scrolling stopped
- after fast (covers fill): at end of phase: 0 visible tiles without cover, 0 bound tiles without cover
- random jumps: at end of phase: 16 visible tiles without cover, 266 bound tiles without cover
- after jumps (covers fill): all visible covers present 15 ms after scrolling stopped
- after jumps (covers fill): at end of phase: 0 visible tiles without cover, 69 bound tiles without cover
- theme switch every 0.5 s: at end of phase: 0 visible tiles without cover, 69 bound tiles without cover
- idle: all visible covers present 0 ms after scrolling stopped
- idle: at end of phase: 0 visible tiles without cover, 69 bound tiles without cover
RSS MiB: process start 23.8, after GTK init 42.9, after building views 63.8, end 229.1, peak 229.1
items materialised by the model: 121358, rows fetched (data built): 121357, wrapped to top: 0
tile widgets created: 385, most tiles bound at once: 385
covers: LoaderStats { requested: 93511, cancelled: 7021, dropped_oldest: 70536, requeued_visible: 1039, decoded: 16993 }, cache hits 27846, stale results 1041, textures cached 600
cover decode+scale per image (worker): mean 1.4 ms, p95 1.5, worst 3.3 (n=16993)
bind-to-cover latency: p50 15.4 ms, p95 21.6, p99 115.7, worst 8303.9 (n=15952)
stylesheet switch to end of next frame: mean 17.1 ms, p95 27.4, worst 27.4 (n=8)
theme log:
  [  0.000 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [  0.000 s] applied Dark, accent #3DAEE9 (startup)
  [  0.001 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  0.001 s] portal accent-color = #3DAEE9
  [ 25.084 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 25.084 s] applied Light, accent #3DAEE9 (scripted override)
  [ 25.589 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 25.589 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 26.095 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 26.095 s] applied Light, accent #3DAEE9 (scripted override)
  [ 26.602 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 26.602 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 27.110 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 27.110 s] applied Light, accent #3DAEE9 (scripted override)
  [ 27.616 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 27.616 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 28.124 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 28.124 s] applied Light, accent #3DAEE9 (scripted override)
  [ 28.630 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 28.630 s] applied Dark, accent #3DAEE9 (scripted override)
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
model+view build 24.6 ms, first frame painted 127 ms after the window was presented
refresh interval reported by compositor: 13.34 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 144 | 13.84 | 13.34 | 16.67 | 83.33 | 1 | 1 | 0.06 | 0.07 | 0.59 | 1.01 | 189.7 |
| slow scroll | 600 | 13.31 | 13.34 | 13.34 | 13.35 | 0 | 0 | 0.63 | 0.92 | 1.20 | 3.77 | 205.2 |
| fast scroll | 449 | 13.34 | 13.34 | 13.34 | 13.34 | 0 | 0 | 5.01 | 6.61 | 7.70 | 8.97 | 261.0 |
| after fast (covers fill) | 149 | 13.34 | 13.34 | 13.34 | 13.34 | 0 | 0 | 0.00 | 0.01 | 0.01 | 0.05 | 261.0 |
| random jumps | 174 | 28.60 | 39.72 | 39.97 | 40.81 | 172 | 0 | 26.73 | 27.56 | 32.09 | 41.74 | 236.7 |
| after jumps (covers fill) | 150 | 13.33 | 13.34 | 13.34 | 13.34 | 0 | 0 | 0.08 | 0.09 | 0.73 | 1.06 | 236.9 |
| theme switch every 0.5 s | 304 | 13.11 | 13.34 | 26.67 | 39.92 | 8 | 0 | 0.75 | 0.36 | 23.82 | 33.51 | 237.5 |
| idle | 149 | 13.34 | 13.34 | 13.34 | 13.34 | 0 | 0 | 0.06 | 0.08 | 0.09 | 0.17 | 237.5 |
- warm-up (idle): at end of phase: 0 visible tiles without cover, 341 bound tiles without cover
- slow scroll: at end of phase: 0 visible tiles without cover, 217 bound tiles without cover
- fast scroll: at end of phase: 0 visible tiles without cover, 0 bound tiles without cover
- after fast (covers fill): all visible covers present 0 ms after scrolling stopped
- after fast (covers fill): at end of phase: 0 visible tiles without cover, 0 bound tiles without cover
- random jumps: at end of phase: 8 visible tiles without cover, 565 bound tiles without cover
- after jumps (covers fill): all visible covers present 13 ms after scrolling stopped
- after jumps (covers fill): at end of phase: 0 visible tiles without cover, 377 bound tiles without cover
- theme switch every 0.5 s: at end of phase: 0 visible tiles without cover, 377 bound tiles without cover
- idle: all visible covers present 0 ms after scrolling stopped
- idle: at end of phase: 0 visible tiles without cover, 377 bound tiles without cover
RSS MiB: process start 24.4, after GTK init 43.5, after building views 68.8, end 237.5, peak 256.2
items materialised by the model: 154290, rows fetched (data built): 154289, wrapped to top: 0
tile widgets created: 641, most tiles bound at once: 641
covers: LoaderStats { requested: 109788, cancelled: 1064, dropped_oldest: 93481, requeued_visible: 1955, decoded: 17198 }, cache hits 44501, stale results 749, textures cached 600
cover decode+scale per image (worker): mean 1.4 ms, p95 1.7, worst 3.1 (n=17198)
bind-to-cover latency: p50 24.9 ms, p95 37.0, p99 10426.1, worst 11490.1 (n=16449)
stylesheet switch to end of next frame: mean 24.7 ms, p95 33.3, worst 33.3 (n=8)
theme log:
  [  0.000 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [  0.000 s] applied Dark, accent #3DAEE9 (startup)
  [  0.001 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  0.001 s] portal accent-color = #3DAEE9
  [ 25.092 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 25.092 s] applied Light, accent #3DAEE9 (scripted override)
  [ 25.597 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 25.597 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 26.104 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 26.104 s] applied Light, accent #3DAEE9 (scripted override)
  [ 26.611 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 26.611 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 27.118 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 27.118 s] applied Light, accent #3DAEE9 (scripted override)
  [ 27.624 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 27.624 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 28.131 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 28.131 s] applied Light, accent #3DAEE9 (scripted override)
  [ 28.638 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 28.638 s] applied Dark, accent #3DAEE9 (scripted override)
shell:
  gtk-decoration-layout = Some("icon:minimize,maximize,close")
  WindowControls start: empty=true, layout=None; end: empty=false, layout=None
  start buttons: []
  end buttons: ["image-button.minimize", "image-button.maximize", "image-button.close"]
  window css classes: ["background", "csd", "hk-dark", "hk"]
  decorated=true titlebar-hidden=true
  gtk-theme-name = Breeze, icon theme = Some("breeze-dark"), font = Some("Noto Sans  10")

