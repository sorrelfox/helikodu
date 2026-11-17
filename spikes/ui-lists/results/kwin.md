## table 200k tracks, 8 columns
GTK 4.24.1, display GdkWaylandDisplay, renderer GskGLRenderer, scale 1 (surface 1.00), window 1440x900, stylesheet on
rows 200000, columns 8
model+view build 21.3 ms, first frame painted 95 ms after the window was presented
refresh interval reported by compositor: 16.67 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 117 | 16.95 | 16.67 | 16.67 | 50.00 | 1 | 0 | 0.00 | 0.00 | 0.01 | 0.01 | 149.2 |
| slow scroll | 481 | 16.62 | 16.67 | 16.68 | 16.68 | 0 | 0 | 1.61 | 2.75 | 3.14 | 3.52 | 149.4 |
| fast scroll | 359 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 3.19 | 4.19 | 4.49 | 4.69 | 149.5 |
| random jumps | 240 | 20.77 | 33.31 | 33.32 | 33.33 | 59 | 0 | 14.69 | 21.37 | 21.66 | 21.81 | 151.1 |
| theme switch every 0.5 s | 262 | 15.27 | 16.67 | 33.31 | 33.32 | 8 | 0 | 0.81 | 0.31 | 24.92 | 26.16 | 151.6 |
| idle | 119 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.00 | 0.00 | 0.00 | 0.05 | 151.6 |
RSS MiB: process start 24.2, after GTK init 43.3, after building views 52.0, end 151.6, peak 151.6
items materialised by the model: 53483, rows fetched (data built): 53482, wrapped to top: 0
cell widgets created: 1640, cell binds: 427856
stylesheet switch to end of next frame: mean 24.7 ms, p95 26.2, worst 26.2 (n=8)
theme log:
  [  0.000 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [  0.000 s] applied Dark, accent #3DAEE9 (startup)
  [  0.001 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  0.001 s] portal accent-color = #3DAEE9
  [ 21.086 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 21.086 s] applied Light, accent #3DAEE9 (scripted override)
  [ 21.605 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 21.605 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 22.122 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 22.122 s] applied Light, accent #3DAEE9 (scripted override)
  [ 22.638 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 22.638 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 23.155 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 23.155 s] applied Light, accent #3DAEE9 (scripted override)
  [ 23.655 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 23.655 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 24.171 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 24.171 s] applied Light, accent #3DAEE9 (scripted override)
  [ 24.689 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 24.689 s] applied Dark, accent #3DAEE9 (scripted override)
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
model+view build 35.1 ms, first frame painted 108 ms after the window was presented
refresh interval reported by compositor: 16.67 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 116 | 17.10 | 16.67 | 16.67 | 66.67 | 1 | 1 | 0.00 | 0.01 | 0.01 | 0.01 | 154.1 |
| slow scroll | 495 | 16.13 | 16.67 | 16.68 | 16.68 | 0 | 0 | 1.75 | 3.01 | 3.33 | 3.81 | 154.3 |
| fast scroll | 359 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 3.54 | 4.35 | 4.43 | 4.57 | 154.3 |
| random jumps | 196 | 25.43 | 33.34 | 33.34 | 33.34 | 103 | 0 | 17.76 | 23.49 | 24.62 | 29.29 | 157.2 |
| theme switch every 0.5 s | 259 | 15.38 | 16.67 | 33.34 | 33.34 | 8 | 0 | 0.90 | 0.28 | 28.94 | 30.09 | 157.5 |
| idle | 119 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.00 | 0.01 | 0.01 | 0.03 | 157.5 |
RSS MiB: process start 24.0, after GTK init 43.1, after building views 54.6, end 157.5, peak 157.5
items materialised by the model: 44352, rows fetched (data built): 44351, wrapped to top: 0
cell widgets created: 2460, cell binds: 532212
stylesheet switch to end of next frame: mean 27.8 ms, p95 30.1, worst 30.1 (n=8)
theme log:
  [  0.000 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [  0.000 s] applied Dark, accent #3DAEE9 (startup)
  [  0.001 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  0.001 s] portal accent-color = #3DAEE9
  [ 21.079 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 21.079 s] applied Light, accent #3DAEE9 (scripted override)
  [ 21.579 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 21.579 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 22.081 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 22.081 s] applied Light, accent #3DAEE9 (scripted override)
  [ 22.581 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 22.581 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 23.080 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 23.080 s] applied Light, accent #3DAEE9 (scripted override)
  [ 23.581 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 23.581 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 24.097 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 24.097 s] applied Light, accent #3DAEE9 (scripted override)
  [ 24.614 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 24.614 s] applied Dark, accent #3DAEE9 (scripted override)
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
model+view build 27.2 ms, first frame painted 100 ms after the window was presented
refresh interval reported by compositor: 16.67 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 117 | 16.95 | 16.67 | 16.67 | 50.00 | 1 | 0 | 0.00 | 0.00 | 0.01 | 0.01 | 147.4 |
| slow scroll | 481 | 16.63 | 16.67 | 16.68 | 16.68 | 0 | 0 | 1.71 | 2.36 | 2.93 | 3.50 | 147.9 |
| fast scroll | 359 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 3.74 | 4.76 | 4.99 | 5.42 | 148.0 |
| random jumps | 290 | 17.19 | 16.91 | 33.31 | 33.33 | 9 | 0 | 13.27 | 18.46 | 18.91 | 18.94 | 149.8 |
| theme switch every 0.5 s | 269 | 14.87 | 16.67 | 16.67 | 33.20 | 1 | 0 | 0.58 | 0.35 | 17.92 | 19.76 | 150.2 |
| idle | 119 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.00 | 0.00 | 0.01 | 0.03 | 150.2 |
RSS MiB: process start 24.2, after GTK init 43.3, after building views 51.8, end 150.2, peak 150.2
items materialised by the model: 65700, rows fetched (data built): 65699, wrapped to top: 0
cell widgets created: 1640, cell binds: 525592
stylesheet switch to end of next frame: mean 18.0 ms, p95 19.8, worst 19.8 (n=8)
theme log:
  [  0.000 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [  0.000 s] applied Dark, accent #3DAEE9 (startup)
  [  0.001 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  0.001 s] portal accent-color = #3DAEE9
  [ 21.078 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 21.078 s] applied Light, accent #3DAEE9 (scripted override)
  [ 21.578 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 21.578 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 22.079 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 22.079 s] applied Light, accent #3DAEE9 (scripted override)
  [ 22.579 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 22.579 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 23.079 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 23.079 s] applied Light, accent #3DAEE9 (scripted override)
  [ 23.581 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 23.581 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 24.096 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 24.096 s] applied Light, accent #3DAEE9 (scripted override)
  [ 24.596 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 24.596 s] applied Dark, accent #3DAEE9 (scripted override)
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
model+view build 14.6 ms, first frame painted 99 ms after the window was presented
refresh interval reported by compositor: 16.67 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 117 | 16.95 | 16.67 | 16.67 | 50.00 | 1 | 0 | 0.00 | 0.01 | 0.01 | 0.01 | 142.5 |
| slow scroll | 481 | 16.63 | 16.68 | 16.68 | 16.68 | 0 | 0 | 0.56 | 0.86 | 1.21 | 2.27 | 142.6 |
| fast scroll | 359 | 16.67 | 16.67 | 16.67 | 16.68 | 0 | 0 | 0.79 | 1.19 | 1.48 | 1.52 | 142.7 |
| random jumps | 299 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 1.02 | 1.45 | 1.54 | 1.71 | 142.7 |
| theme switch every 0.5 s | 242 | 16.46 | 16.67 | 16.77 | 16.83 | 0 | 0 | 0.60 | 0.33 | 16.53 | 16.82 | 142.8 |
| idle | 119 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.00 | 0.00 | 0.01 | 0.04 | 142.8 |
RSS MiB: process start 24.1, after GTK init 43.1, after building views 47.7, end 142.8, peak 142.8
items materialised by the model: 101, rows fetched (data built): 100, wrapped to top: 41
cell widgets created: 800, cell binds: 800
stylesheet switch to end of next frame: mean 16.5 ms, p95 16.8, worst 16.8 (n=8)
theme log:
  [  0.000 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [  0.000 s] applied Dark, accent #3DAEE9 (startup)
  [  0.001 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  0.001 s] portal accent-color = #3DAEE9
  [ 21.076 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 21.076 s] applied Light, accent #3DAEE9 (scripted override)
  [ 21.576 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 21.576 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 22.076 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 22.076 s] applied Light, accent #3DAEE9 (scripted override)
  [ 22.576 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 22.576 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 23.077 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 23.077 s] applied Light, accent #3DAEE9 (scripted override)
  [ 23.593 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 23.593 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 24.093 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 24.093 s] applied Light, accent #3DAEE9 (scripted override)
  [ 24.593 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 24.593 s] applied Dark, accent #3DAEE9 (scripted override)
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
model+view build 13.4 ms, first frame painted 105 ms after the window was presented
refresh interval reported by compositor: 16.67 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 118 | 16.95 | 16.67 | 16.67 | 50.00 | 1 | 0 | 0.04 | 0.02 | 0.36 | 1.69 | 179.0 |
| slow scroll | 481 | 16.60 | 16.68 | 16.68 | 16.68 | 0 | 0 | 0.60 | 0.93 | 1.10 | 4.79 | 192.4 |
| fast scroll | 359 | 16.67 | 16.67 | 16.67 | 16.68 | 0 | 0 | 3.45 | 5.49 | 7.35 | 8.01 | 228.3 |
| after fast (covers fill) | 119 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.00 | 0.00 | 0.00 | 0.05 | 228.3 |
| random jumps | 225 | 22.08 | 33.26 | 33.36 | 33.65 | 73 | 0 | 17.52 | 19.09 | 22.58 | 25.58 | 222.7 |
| after jumps (covers fill) | 120 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.04 | 0.03 | 0.79 | 0.91 | 222.3 |
| theme switch every 0.5 s | 244 | 16.39 | 16.67 | 33.30 | 33.31 | 6 | 0 | 0.72 | 0.28 | 19.26 | 29.05 | 223.1 |
| idle | 119 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.03 | 0.03 | 0.03 | 0.12 | 223.1 |
- warm-up (idle): at end of phase: 0 visible tiles without cover, 104 bound tiles without cover
- slow scroll: at end of phase: 0 visible tiles without cover, 0 bound tiles without cover
- fast scroll: at end of phase: 0 visible tiles without cover, 0 bound tiles without cover
- after fast (covers fill): all visible covers present 0 ms after scrolling stopped
- after fast (covers fill): at end of phase: 0 visible tiles without cover, 0 bound tiles without cover
- random jumps: at end of phase: 15 visible tiles without cover, 293 bound tiles without cover
- after jumps (covers fill): all visible covers present 17 ms after scrolling stopped
- after jumps (covers fill): at end of phase: 0 visible tiles without cover, 103 bound tiles without cover
- theme switch every 0.5 s: at end of phase: 0 visible tiles without cover, 103 bound tiles without cover
- idle: all visible covers present 0 ms after scrolling stopped
- idle: at end of phase: 0 visible tiles without cover, 103 bound tiles without cover
RSS MiB: process start 24.4, after GTK init 43.3, after building views 62.5, end 223.1, peak 224.3
items materialised by the model: 104483, rows fetched (data built): 104482, wrapped to top: 0
tile widgets created: 385, most tiles bound at once: 385
covers: LoaderStats { requested: 85298, cancelled: 7538, dropped_oldest: 61727, requeued_visible: 971, decoded: 17004 }, cache hits 19184, stale results 964, textures cached 600
cover decode+scale per image (worker): mean 1.4 ms, p95 1.7, worst 2.9 (n=17004)
bind-to-cover latency: p50 16.1 ms, p95 28.5, p99 110.4, worst 8582.9 (n=16040)
stylesheet switch to end of next frame: mean 19.9 ms, p95 29.0, worst 29.0 (n=8)
theme log:
  [  0.000 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [  0.000 s] applied Dark, accent #3DAEE9 (startup)
  [  0.001 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  0.002 s] portal accent-color = #3DAEE9
  [ 25.104 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 25.104 s] applied Light, accent #3DAEE9 (scripted override)
  [ 25.619 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 25.619 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 26.119 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 26.119 s] applied Light, accent #3DAEE9 (scripted override)
  [ 26.620 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 26.620 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 27.137 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 27.137 s] applied Light, accent #3DAEE9 (scripted override)
  [ 27.653 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 27.653 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 28.153 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 28.153 s] applied Light, accent #3DAEE9 (scripted override)
  [ 28.670 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 28.671 s] applied Dark, accent #3DAEE9 (scripted override)
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
model+view build 16.9 ms, first frame painted 119 ms after the window was presented
refresh interval reported by compositor: 16.67 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 117 | 17.09 | 16.67 | 16.67 | 66.67 | 1 | 1 | 0.05 | 0.03 | 0.82 | 1.68 | 184.9 |
| slow scroll | 492 | 16.23 | 16.67 | 16.68 | 16.68 | 0 | 0 | 0.72 | 1.04 | 1.34 | 3.23 | 199.7 |
| fast scroll | 359 | 16.67 | 16.67 | 16.67 | 16.68 | 0 | 0 | 4.68 | 7.30 | 7.57 | 8.40 | 257.1 |
| after fast (covers fill) | 119 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.00 | 0.00 | 0.01 | 0.03 | 257.1 |
| random jumps | 150 | 33.11 | 33.33 | 33.49 | 49.99 | 147 | 0 | 28.90 | 31.40 | 34.40 | 36.91 | 232.7 |
| after jumps (covers fill) | 119 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.10 | 0.08 | 0.98 | 1.99 | 232.8 |
| theme switch every 0.5 s | 259 | 15.38 | 16.67 | 33.33 | 33.46 | 8 | 0 | 0.89 | 0.34 | 24.86 | 34.11 | 233.4 |
| idle | 119 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.06 | 0.07 | 0.18 | 0.20 | 233.4 |
- warm-up (idle): at end of phase: 0 visible tiles without cover, 349 bound tiles without cover
- slow scroll: at end of phase: 0 visible tiles without cover, 221 bound tiles without cover
- fast scroll: at end of phase: 0 visible tiles without cover, 0 bound tiles without cover
- after fast (covers fill): all visible covers present 0 ms after scrolling stopped
- after fast (covers fill): at end of phase: 0 visible tiles without cover, 0 bound tiles without cover
- random jumps: at end of phase: 15 visible tiles without cover, 540 bound tiles without cover
- after jumps (covers fill): all visible covers present 16 ms after scrolling stopped
- after jumps (covers fill): at end of phase: 0 visible tiles without cover, 350 bound tiles without cover
- theme switch every 0.5 s: at end of phase: 0 visible tiles without cover, 350 bound tiles without cover
- idle: all visible covers present 0 ms after scrolling stopped
- idle: at end of phase: 0 visible tiles without cover, 350 bound tiles without cover
RSS MiB: process start 24.0, after GTK init 42.9, after building views 64.3, end 233.4, peak 253.4
items materialised by the model: 125682, rows fetched (data built): 125681, wrapped to top: 0
tile widgets created: 641, most tiles bound at once: 641
covers: LoaderStats { requested: 94827, cancelled: 968, dropped_oldest: 78271, requeued_visible: 1655, decoded: 17243 }, cache hits 30854, stale results 690, textures cached 600
cover decode+scale per image (worker): mean 1.4 ms, p95 1.5, worst 3.1 (n=17243)
bind-to-cover latency: p50 26.4 ms, p95 40.3, p99 10470.1, worst 11554.7 (n=16553)
stylesheet switch to end of next frame: mean 25.1 ms, p95 33.9, worst 33.9 (n=8)
theme log:
  [  0.000 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [  0.000 s] applied Dark, accent #3DAEE9 (startup)
  [  0.001 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  0.001 s] portal accent-color = #3DAEE9
  [ 25.086 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 25.086 s] applied Light, accent #3DAEE9 (scripted override)
  [ 25.587 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 25.587 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 26.104 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 26.104 s] applied Light, accent #3DAEE9 (scripted override)
  [ 26.621 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 26.621 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 27.137 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 27.137 s] applied Light, accent #3DAEE9 (scripted override)
  [ 27.654 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 27.654 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 28.170 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 28.170 s] applied Light, accent #3DAEE9 (scripted override)
  [ 28.686 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 28.686 s] applied Dark, accent #3DAEE9 (scripted override)
shell:
  gtk-decoration-layout = Some("icon:minimize,maximize,close")
  WindowControls start: empty=true, layout=None; end: empty=false, layout=None
  start buttons: []
  end buttons: ["image-button.minimize", "image-button.maximize", "image-button.close"]
  window css classes: ["background", "csd", "hk-dark", "hk"]
  decorated=true titlebar-hidden=true
  gtk-theme-name = Breeze, icon theme = Some("breeze-dark"), font = Some("Noto Sans  10")

