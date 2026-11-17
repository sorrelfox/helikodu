## table 200k tracks, 8 columns
GTK 4.24.1, display GdkWaylandDisplay, renderer GskGLRenderer, scale 1 (surface 1.00), window 1440x900, stylesheet on
rows 200000, columns 8
model+view build 23.5 ms, first frame painted 100 ms after the window was presented
refresh interval reported by compositor: 16.67 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 116 | 17.10 | 16.67 | 16.67 | 66.67 | 1 | 1 | 0.00 | 0.01 | 0.01 | 0.01 | 150.9 |
| slow scroll | 491 | 16.27 | 16.67 | 16.67 | 17.03 | 0 | 0 | 1.49 | 2.42 | 2.68 | 3.19 | 151.2 |
| fast scroll | 360 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 2.79 | 3.46 | 3.63 | 4.11 | 151.3 |
| random jumps | 299 | 16.67 | 16.70 | 16.75 | 16.79 | 0 | 0 | 12.59 | 17.03 | 17.22 | 17.63 | 152.9 |
| theme switch every 0.5 s | 244 | 16.37 | 16.71 | 20.44 | 30.04 | 1 | 0 | 0.77 | 0.32 | 23.63 | 25.03 | 153.4 |
| idle | 119 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.00 | 0.01 | 0.01 | 0.06 | 153.4 |
RSS MiB: process start 24.6, after GTK init 45.3, after building views 53.2, end 153.4, peak 153.4
items materialised by the model: 65608, rows fetched (data built): 65607, wrapped to top: 0
cell widgets created: 1640, cell binds: 524856
stylesheet switch to end of next frame: mean 22.1 ms, p95 25.0, worst 25.0 (n=8)
theme log:
  [  0.000 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [  0.000 s] applied Dark, accent #3DAEE9 (startup)
  [  0.001 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  0.001 s] portal accent-color = #3DAEE9
  [ 21.075 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 21.075 s] applied Light, accent #3DAEE9 (scripted override)
  [ 21.576 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 21.576 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 22.079 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 22.079 s] applied Light, accent #3DAEE9 (scripted override)
  [ 22.583 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 22.583 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 23.086 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 23.086 s] applied Light, accent #3DAEE9 (scripted override)
  [ 23.591 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 23.591 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 24.100 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 24.100 s] applied Light, accent #3DAEE9 (scripted override)
  [ 24.602 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 24.602 s] applied Dark, accent #3DAEE9 (scripted override)
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
model+view build 31.2 ms, first frame painted 110 ms after the window was presented
refresh interval reported by compositor: 16.67 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 116 | 17.10 | 16.67 | 16.67 | 66.67 | 1 | 1 | 0.00 | 0.00 | 0.01 | 0.01 | 155.6 |
| slow scroll | 491 | 16.26 | 16.67 | 16.67 | 16.67 | 0 | 0 | 1.83 | 3.12 | 3.38 | 4.63 | 155.9 |
| fast scroll | 359 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 3.33 | 4.41 | 4.81 | 5.34 | 156.1 |
| random jumps | 250 | 19.99 | 30.72 | 30.84 | 31.01 | 50 | 0 | 17.49 | 22.87 | 23.27 | 23.63 | 158.9 |
| theme switch every 0.5 s | 238 | 16.77 | 16.70 | 33.15 | 33.31 | 8 | 0 | 1.01 | 0.35 | 29.74 | 30.72 | 159.3 |
| idle | 119 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.00 | 0.01 | 0.01 | 0.04 | 159.3 |
RSS MiB: process start 24.2, after GTK init 44.9, after building views 55.6, end 159.3, peak 159.3
items materialised by the model: 55558, rows fetched (data built): 55557, wrapped to top: 0
cell widgets created: 2460, cell binds: 666684
stylesheet switch to end of next frame: mean 28.3 ms, p95 30.7, worst 30.7 (n=8)
theme log:
  [  0.000 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [  0.000 s] applied Dark, accent #3DAEE9 (startup)
  [  0.001 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  0.001 s] portal accent-color = #3DAEE9
  [ 21.088 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 21.088 s] applied Light, accent #3DAEE9 (scripted override)
  [ 21.600 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 21.600 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 22.115 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 22.115 s] applied Light, accent #3DAEE9 (scripted override)
  [ 22.632 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 22.632 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 23.148 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 23.148 s] applied Light, accent #3DAEE9 (scripted override)
  [ 23.649 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 23.649 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 24.164 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 24.164 s] applied Light, accent #3DAEE9 (scripted override)
  [ 24.680 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 24.680 s] applied Dark, accent #3DAEE9 (scripted override)
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
model+view build 21.7 ms, first frame painted 95 ms after the window was presented
refresh interval reported by compositor: 16.67 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 116 | 17.10 | 16.67 | 16.67 | 66.67 | 1 | 1 | 0.00 | 0.01 | 0.01 | 0.01 | 148.8 |
| slow scroll | 481 | 16.60 | 16.68 | 16.73 | 16.82 | 0 | 0 | 1.65 | 2.24 | 2.51 | 2.79 | 149.4 |
| fast scroll | 359 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 3.28 | 3.82 | 4.20 | 4.74 | 149.6 |
| random jumps | 298 | 16.75 | 16.94 | 17.56 | 31.87 | 1 | 0 | 12.59 | 17.69 | 19.89 | 20.35 | 151.3 |
| theme switch every 0.5 s | 267 | 14.92 | 16.67 | 16.68 | 16.70 | 0 | 0 | 0.55 | 0.32 | 17.03 | 18.01 | 151.8 |
| idle | 119 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.00 | 0.00 | 0.01 | 0.04 | 151.8 |
RSS MiB: process start 24.8, after GTK init 45.2, after building views 53.1, end 151.8, peak 151.8
items materialised by the model: 67308, rows fetched (data built): 67307, wrapped to top: 0
cell widgets created: 1640, cell binds: 538456
stylesheet switch to end of next frame: mean 17.1 ms, p95 18.0, worst 18.0 (n=8)
theme log:
  [  0.000 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [  0.000 s] applied Dark, accent #3DAEE9 (startup)
  [  0.001 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  0.001 s] portal accent-color = #3DAEE9
  [ 21.068 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 21.068 s] applied Light, accent #3DAEE9 (scripted override)
  [ 21.571 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 21.571 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 22.069 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 22.069 s] applied Light, accent #3DAEE9 (scripted override)
  [ 22.571 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 22.571 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 23.069 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 23.069 s] applied Light, accent #3DAEE9 (scripted override)
  [ 23.571 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 23.571 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 24.071 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 24.071 s] applied Light, accent #3DAEE9 (scripted override)
  [ 24.572 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 24.572 s] applied Dark, accent #3DAEE9 (scripted override)
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
model+view build 11.7 ms, first frame painted 89 ms after the window was presented
refresh interval reported by compositor: 16.67 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 117 | 16.95 | 16.67 | 16.67 | 50.00 | 1 | 0 | 0.00 | 0.01 | 0.01 | 0.01 | 144.2 |
| slow scroll | 495 | 16.13 | 16.67 | 16.68 | 16.79 | 0 | 0 | 0.52 | 0.86 | 1.20 | 1.96 | 144.5 |
| fast scroll | 359 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.78 | 1.20 | 1.44 | 1.49 | 144.6 |
| random jumps | 299 | 16.67 | 16.67 | 16.67 | 16.69 | 0 | 0 | 0.98 | 1.52 | 2.00 | 2.67 | 144.6 |
| theme switch every 0.5 s | 270 | 14.76 | 16.67 | 16.78 | 17.07 | 0 | 0 | 0.52 | 0.29 | 17.04 | 17.57 | 144.7 |
| idle | 119 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.00 | 0.01 | 0.01 | 0.04 | 143.8 |
RSS MiB: process start 24.8, after GTK init 45.4, after building views 49.2, end 143.9, peak 143.9
items materialised by the model: 101, rows fetched (data built): 100, wrapped to top: 41
cell widgets created: 800, cell binds: 800
stylesheet switch to end of next frame: mean 15.9 ms, p95 17.6, worst 17.6 (n=8)
theme log:
  [  0.000 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [  0.000 s] applied Dark, accent #3DAEE9 (startup)
  [  0.001 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  0.001 s] portal accent-color = #3DAEE9
  [ 21.057 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 21.057 s] applied Light, accent #3DAEE9 (scripted override)
  [ 21.559 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 21.559 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 22.058 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 22.058 s] applied Light, accent #3DAEE9 (scripted override)
  [ 22.560 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 22.560 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 23.059 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 23.059 s] applied Light, accent #3DAEE9 (scripted override)
  [ 23.559 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 23.559 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 24.076 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 24.076 s] applied Light, accent #3DAEE9 (scripted override)
  [ 24.576 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 24.576 s] applied Dark, accent #3DAEE9 (scripted override)
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
model+view build 11.2 ms, first frame painted 105 ms after the window was presented
refresh interval reported by compositor: 16.67 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 116 | 17.24 | 16.67 | 16.67 | 83.33 | 1 | 1 | 0.03 | 0.02 | 0.54 | 0.91 | 178.9 |
| slow scroll | 492 | 16.24 | 16.68 | 16.84 | 17.46 | 0 | 0 | 0.56 | 0.88 | 1.23 | 4.34 | 192.4 |
| fast scroll | 360 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 3.24 | 5.21 | 5.78 | 6.58 | 226.0 |
| after fast (covers fill) | 119 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.00 | 0.00 | 0.01 | 0.19 | 226.0 |
| random jumps | 297 | 16.80 | 16.98 | 18.25 | 32.62 | 1 | 0 | 16.05 | 16.65 | 19.55 | 20.76 | 223.9 |
| after jumps (covers fill) | 120 | 16.53 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.04 | 0.18 | 0.42 | 0.95 | 223.9 |
| theme switch every 0.5 s | 263 | 15.16 | 16.67 | 17.13 | 32.17 | 1 | 0 | 0.66 | 0.35 | 18.40 | 27.94 | 224.5 |
| idle | 119 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.02 | 0.03 | 0.04 | 0.08 | 224.5 |
- warm-up (idle): at end of phase: 0 visible tiles without cover, 104 bound tiles without cover
- slow scroll: at end of phase: 0 visible tiles without cover, 0 bound tiles without cover
- fast scroll: at end of phase: 0 visible tiles without cover, 0 bound tiles without cover
- after fast (covers fill): all visible covers present 0 ms after scrolling stopped
- after fast (covers fill): at end of phase: 0 visible tiles without cover, 0 bound tiles without cover
- random jumps: at end of phase: 16 visible tiles without cover, 341 bound tiles without cover
- after jumps (covers fill): all visible covers present 16 ms after scrolling stopped
- after jumps (covers fill): at end of phase: 0 visible tiles without cover, 125 bound tiles without cover
- theme switch every 0.5 s: at end of phase: 0 visible tiles without cover, 125 bound tiles without cover
- idle: all visible covers present 0 ms after scrolling stopped
- idle: at end of phase: 0 visible tiles without cover, 125 bound tiles without cover
RSS MiB: process start 24.2, after GTK init 43.4, after building views 63.7, end 224.5, peak 224.5
items materialised by the model: 132265, rows fetched (data built): 132264, wrapped to top: 0
tile widgets created: 385, most tiles bound at once: 385
covers: LoaderStats { requested: 112007, cancelled: 5850, dropped_oldest: 90221, requeued_visible: 1205, decoded: 17141 }, cache hits 20257, stale results 1243, textures cached 600
cover decode+scale per image (worker): mean 1.4 ms, p95 1.7, worst 3.7 (n=17141)
bind-to-cover latency: p50 15.1 ms, p95 18.3, p99 110.4, worst 8573.0 (n=15898)
stylesheet switch to end of next frame: mean 19.6 ms, p95 27.8, worst 27.8 (n=8)
theme log:
  [  0.000 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [  0.000 s] applied Dark, accent #3DAEE9 (startup)
  [  0.002 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  0.002 s] portal accent-color = #3DAEE9
  [ 25.100 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 25.100 s] applied Light, accent #3DAEE9 (scripted override)
  [ 25.616 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 25.616 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 26.116 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 26.116 s] applied Light, accent #3DAEE9 (scripted override)
  [ 26.616 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 26.616 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 27.117 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 27.117 s] applied Light, accent #3DAEE9 (scripted override)
  [ 27.618 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 27.618 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 28.118 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 28.118 s] applied Light, accent #3DAEE9 (scripted override)
  [ 28.619 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 28.619 s] applied Dark, accent #3DAEE9 (scripted override)
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
model+view build 16.0 ms, first frame painted 119 ms after the window was presented
refresh interval reported by compositor: 16.67 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 116 | 17.23 | 16.67 | 16.67 | 83.33 | 1 | 1 | 0.06 | 0.07 | 0.84 | 0.90 | 184.1 |
| slow scroll | 492 | 16.24 | 16.68 | 16.87 | 18.85 | 0 | 0 | 0.63 | 0.97 | 1.15 | 3.86 | 200.5 |
| fast scroll | 359 | 16.67 | 16.67 | 16.68 | 16.68 | 0 | 0 | 4.61 | 7.93 | 8.84 | 9.54 | 257.4 |
| after fast (covers fill) | 119 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.00 | 0.01 | 0.01 | 0.04 | 257.4 |
| random jumps | 184 | 27.04 | 33.20 | 33.42 | 47.24 | 110 | 0 | 26.45 | 27.93 | 32.21 | 40.99 | 230.9 |
| after jumps (covers fill) | 120 | 16.53 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.08 | 0.23 | 0.67 | 1.07 | 230.9 |
| theme switch every 0.5 s | 241 | 16.58 | 16.71 | 29.59 | 33.70 | 5 | 0 | 0.91 | 0.39 | 23.83 | 33.92 | 231.7 |
| idle | 119 | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 0 | 0.06 | 0.09 | 0.10 | 0.26 | 231.7 |
- warm-up (idle): at end of phase: 0 visible tiles without cover, 349 bound tiles without cover
- slow scroll: at end of phase: 0 visible tiles without cover, 226 bound tiles without cover
- fast scroll: at end of phase: 0 visible tiles without cover, 0 bound tiles without cover
- after fast (covers fill): all visible covers present 0 ms after scrolling stopped
- after fast (covers fill): at end of phase: 0 visible tiles without cover, 0 bound tiles without cover
- random jumps: at end of phase: 16 visible tiles without cover, 559 bound tiles without cover
- after jumps (covers fill): all visible covers present 17 ms after scrolling stopped
- after jumps (covers fill): at end of phase: 0 visible tiles without cover, 362 bound tiles without cover
- theme switch every 0.5 s: at end of phase: 0 visible tiles without cover, 362 bound tiles without cover
- idle: all visible covers present 0 ms after scrolling stopped
- idle: at end of phase: 0 visible tiles without cover, 362 bound tiles without cover
RSS MiB: process start 24.0, after GTK init 43.2, after building views 64.3, end 231.7, peak 253.1
items materialised by the model: 147512, rows fetched (data built): 147511, wrapped to top: 0
tile widgets created: 641, most tiles bound at once: 641
covers: LoaderStats { requested: 116074, cancelled: 1117, dropped_oldest: 99842, requeued_visible: 2097, decoded: 17212 }, cache hits 31437, stale results 779, textures cached 600
cover decode+scale per image (worker): mean 1.4 ms, p95 1.7, worst 3.1 (n=17212)
bind-to-cover latency: p50 24.8 ms, p95 37.5, p99 10503.0, worst 11546.4 (n=16433)
stylesheet switch to end of next frame: mean 24.7 ms, p95 33.8, worst 33.8 (n=8)
theme log:
  [  0.000 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [  0.000 s] applied Dark, accent #3DAEE9 (startup)
  [  0.001 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  0.001 s] portal accent-color = #3DAEE9
  [ 25.090 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 25.090 s] applied Light, accent #3DAEE9 (scripted override)
  [ 25.591 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 25.591 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 26.100 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 26.100 s] applied Light, accent #3DAEE9 (scripted override)
  [ 26.605 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 26.605 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 27.117 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 27.117 s] applied Light, accent #3DAEE9 (scripted override)
  [ 27.631 s] GtkSettings: color-scheme=dark, accent=#3DAEE9
  [ 27.631 s] applied Dark, accent #3DAEE9 (scripted override)
  [ 28.134 s] GtkSettings: color-scheme=light, accent=#3DAEE9
  [ 28.134 s] applied Light, accent #3DAEE9 (scripted override)
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

