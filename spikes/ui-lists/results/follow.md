## table stand-in portal, button layout appmenu:close
GTK 4.24.1, display GdkWaylandDisplay, renderer GskGLRenderer, scale 1 (surface 1.00), window 1440x900, stylesheet on
rows 200000, columns 8
model+view build 21.0 ms, first frame painted 103 ms after the window was presented
refresh interval reported by compositor: 16.67 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 118 | 16.95 | 16.67 | 16.67 | 50.00 | 1 | 0 | 0.20 | 0.50 | 0.65 | 14.14 | 147.7 |
| following the portal | 479 | 16.68 | 16.67 | 16.67 | 19.24 | 0 | 0 | 0.25 | 0.53 | 1.49 | 16.68 | 147.9 |
RSS MiB: process start 24.4, after GTK init 40.1, after building views 48.3, end 147.9, peak 147.9
items materialised by the model: 206, rows fetched (data built): 205, wrapped to top: 0
cell widgets created: 1640, cell binds: 1640
stylesheet switch to end of next frame: mean 14.8 ms, p95 16.9, worst 16.9 (n=6)
theme log:
  [  0.000 s] GtkSettings: color-scheme=light, accent=#3584E4
  [  0.000 s] applied Light, accent #3584E4 (startup)
  [  0.000 s] portal color-scheme = Some(2) (0 none, 1 dark, 2 light)
  [  0.001 s] portal accent-color = #3584E4
  [  1.371 s] GtkSettings: color-scheme=dark, accent=#3584E4
  [  1.371 s] applied Dark, accent #3584E4 (GtkSettings gtk-interface-color-scheme changed)
  [  1.371 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  1.371 s] GtkSettings: color-scheme=dark, accent=#F0B23C
  [  1.371 s] applied Dark, accent #F0B23C (GtkSettings gtk-accent-color changed)
  [  1.371 s] portal accent-color = #F0B23C
  [  2.874 s] GtkSettings: color-scheme=light, accent=#F0B23C
  [  2.874 s] applied Light, accent #F0B23C (GtkSettings gtk-interface-color-scheme changed)
  [  2.874 s] portal color-scheme = Some(2) (0 none, 1 dark, 2 light)
  [  2.874 s] GtkSettings: color-scheme=light, accent=#3584E4
  [  2.874 s] applied Light, accent #3584E4 (GtkSettings gtk-accent-color changed)
  [  2.874 s] portal accent-color = #3584E4
  [  4.374 s] GtkSettings: color-scheme=dark, accent=#3584E4
  [  4.374 s] applied Dark, accent #3584E4 (GtkSettings gtk-interface-color-scheme changed)
  [  4.374 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  4.374 s] GtkSettings: color-scheme=dark, accent=#F0B23C
  [  4.374 s] applied Dark, accent #F0B23C (GtkSettings gtk-accent-color changed)
  [  4.374 s] portal accent-color = #F0B23C
  [  5.876 s] GtkSettings: color-scheme=light, accent=#F0B23C
  [  5.876 s] applied Light, accent #F0B23C (GtkSettings gtk-interface-color-scheme changed)
  [  5.876 s] portal color-scheme = Some(2) (0 none, 1 dark, 2 light)
  [  5.876 s] GtkSettings: color-scheme=light, accent=#3584E4
  [  5.876 s] applied Light, accent #3584E4 (GtkSettings gtk-accent-color changed)
  [  5.876 s] portal accent-color = #3584E4
  [  7.377 s] GtkSettings: color-scheme=dark, accent=#3584E4
  [  7.377 s] applied Dark, accent #3584E4 (GtkSettings gtk-interface-color-scheme changed)
  [  7.377 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  7.377 s] GtkSettings: color-scheme=dark, accent=#F0B23C
  [  7.377 s] applied Dark, accent #F0B23C (GtkSettings gtk-accent-color changed)
  [  7.377 s] portal accent-color = #F0B23C
  [  8.879 s] GtkSettings: color-scheme=light, accent=#F0B23C
  [  8.879 s] applied Light, accent #F0B23C (GtkSettings gtk-interface-color-scheme changed)
  [  8.879 s] portal color-scheme = Some(2) (0 none, 1 dark, 2 light)
  [  8.879 s] GtkSettings: color-scheme=light, accent=#3584E4
  [  8.879 s] applied Light, accent #3584E4 (GtkSettings gtk-accent-color changed)
  [  8.879 s] portal accent-color = #3584E4
shell:
  gtk-decoration-layout = Some("appmenu:close")
  WindowControls start: empty=true, layout=None; end: empty=false, layout=None
  start buttons: []
  end buttons: ["image-button.close"]
  window css classes: ["background", "csd", "hk-light", "hk"]
  decorated=true titlebar-hidden=true
  gtk-theme-name = Adwaita, icon theme = Some("gnome"), font = Some("Adwaita Sans 11")

## table stand-in portal, button layout close,minimize,maximize:
GTK 4.24.1, display GdkWaylandDisplay, renderer GskGLRenderer, scale 1 (surface 1.00), window 1440x900, stylesheet on
rows 200000, columns 8
model+view build 21.3 ms, first frame painted 108 ms after the window was presented
refresh interval reported by compositor: 16.67 ms
| phase | frames | interval mean | p95 | p99 | worst | >1.5x refresh | >50 ms | work mean | work p95 | work p99 | work worst | RSS end MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| warm-up (idle) | 116 | 17.10 | 16.67 | 16.74 | 66.67 | 1 | 1 | 0.19 | 0.22 | 1.00 | 16.37 | 147.7 |
| following the portal | 479 | 16.67 | 16.67 | 16.67 | 18.35 | 0 | 0 | 0.27 | 0.53 | 1.64 | 17.91 | 147.7 |
RSS MiB: process start 24.3, after GTK init 40.2, after building views 48.3, end 147.7, peak 147.7
items materialised by the model: 206, rows fetched (data built): 205, wrapped to top: 0
cell widgets created: 1640, cell binds: 1640
stylesheet switch to end of next frame: mean 16.7 ms, p95 19.7, worst 19.7 (n=6)
theme log:
  [  0.000 s] GtkSettings: color-scheme=light, accent=#3584E4
  [  0.000 s] applied Light, accent #3584E4 (startup)
  [  0.000 s] portal color-scheme = Some(2) (0 none, 1 dark, 2 light)
  [  0.001 s] portal accent-color = #3584E4
  [  1.374 s] GtkSettings: color-scheme=dark, accent=#3584E4
  [  1.374 s] applied Dark, accent #3584E4 (GtkSettings gtk-interface-color-scheme changed)
  [  1.374 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  1.374 s] GtkSettings: color-scheme=dark, accent=#F0B23C
  [  1.374 s] applied Dark, accent #F0B23C (GtkSettings gtk-accent-color changed)
  [  1.374 s] portal accent-color = #F0B23C
  [  2.874 s] GtkSettings: color-scheme=light, accent=#F0B23C
  [  2.874 s] applied Light, accent #F0B23C (GtkSettings gtk-interface-color-scheme changed)
  [  2.874 s] portal color-scheme = Some(2) (0 none, 1 dark, 2 light)
  [  2.875 s] GtkSettings: color-scheme=light, accent=#3584E4
  [  2.875 s] applied Light, accent #3584E4 (GtkSettings gtk-accent-color changed)
  [  2.875 s] portal accent-color = #3584E4
  [  4.376 s] GtkSettings: color-scheme=dark, accent=#3584E4
  [  4.376 s] applied Dark, accent #3584E4 (GtkSettings gtk-interface-color-scheme changed)
  [  4.376 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  4.377 s] GtkSettings: color-scheme=dark, accent=#F0B23C
  [  4.377 s] applied Dark, accent #F0B23C (GtkSettings gtk-accent-color changed)
  [  4.377 s] portal accent-color = #F0B23C
  [  5.878 s] GtkSettings: color-scheme=light, accent=#F0B23C
  [  5.878 s] applied Light, accent #F0B23C (GtkSettings gtk-interface-color-scheme changed)
  [  5.878 s] portal color-scheme = Some(2) (0 none, 1 dark, 2 light)
  [  5.878 s] GtkSettings: color-scheme=light, accent=#3584E4
  [  5.878 s] applied Light, accent #3584E4 (GtkSettings gtk-accent-color changed)
  [  5.878 s] portal accent-color = #3584E4
  [  7.379 s] GtkSettings: color-scheme=dark, accent=#3584E4
  [  7.379 s] applied Dark, accent #3584E4 (GtkSettings gtk-interface-color-scheme changed)
  [  7.379 s] portal color-scheme = Some(1) (0 none, 1 dark, 2 light)
  [  7.379 s] GtkSettings: color-scheme=dark, accent=#F0B23C
  [  7.380 s] applied Dark, accent #F0B23C (GtkSettings gtk-accent-color changed)
  [  7.380 s] portal accent-color = #F0B23C
  [  8.880 s] GtkSettings: color-scheme=light, accent=#F0B23C
  [  8.880 s] applied Light, accent #F0B23C (GtkSettings gtk-interface-color-scheme changed)
  [  8.880 s] portal color-scheme = Some(2) (0 none, 1 dark, 2 light)
  [  8.881 s] GtkSettings: color-scheme=light, accent=#3584E4
  [  8.881 s] applied Light, accent #3584E4 (GtkSettings gtk-accent-color changed)
  [  8.881 s] portal accent-color = #3584E4
shell:
  gtk-decoration-layout = Some("close,minimize,maximize:")
  WindowControls start: empty=false, layout=None; end: empty=true, layout=None
  start buttons: ["image-button.close", "image-button.minimize", "image-button.maximize"]
  end buttons: []
  window css classes: ["background", "csd", "hk-light", "hk"]
  decorated=true titlebar-hidden=true
  gtk-theme-name = Adwaita, icon theme = Some("gnome"), font = Some("Adwaita Sans 11")

