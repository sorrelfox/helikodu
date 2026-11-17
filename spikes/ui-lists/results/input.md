## window shell with injected pointer input (mutter, headless)
- gtk-decoration-layout = Some("icon:minimize,maximize,close")
- WindowControls start: empty=true, layout=None; end: empty=false, layout=None
- start buttons: []
- end buttons: ["image-button.minimize", "image-button.maximize", "image-button.close"]
- window css classes: ["background", "csd", "hk-dark", "hk", "resizable-top", "resizable-right", "resizable-bottom", "resizable-left"]
- decorated=true titlebar-hidden=true
- gtk-theme-name = Breeze, icon theme = Some("breeze-dark"), font = Some("Noto Sans  10")
- double-click on content header strip: maximized false -> true (works)
- double-click again: maximized -> false
- drag on panel strip by (-150, +80): pointer now at window (1327, 23), pressed at (1326, 23) -> window moved with the pointer (works)
- maximize button: maximized false -> true
- maximize button again: maximized true -> false
- close button: close-request received = true
- minimize button: toplevel state changes afterwards ["ToplevelState(TOP_RESIZABLE | RIGHT_RESIZABLE | BOTTOM_RESIZABLE | LEFT_RESIZABLE)"], suspended now = false

## window shell with injected pointer input (gnome-shell, headless)
- gtk-decoration-layout = Some("icon:minimize,maximize,close")
- WindowControls start: empty=true, layout=None; end: empty=false, layout=None
- start buttons: []
- end buttons: ["image-button.minimize", "image-button.maximize", "image-button.close"]
- window css classes: ["background", "csd", "hk-dark", "hk", "resizable-top", "resizable-right", "resizable-bottom", "resizable-left"]
- decorated=true titlebar-hidden=true
- gtk-theme-name = Breeze, icon theme = Some("breeze-dark"), font = Some("Noto Sans  10")
- window did not see the pointer at the screen centre
- double-click on content header strip: maximized false -> true (works)
- double-click again: maximized -> false
- drag on panel strip by (-150, +80): pointer now at window (1327, 27), pressed at (1326, 23) -> window moved with the pointer (works)
- maximize button: maximized false -> true
- maximize button again: maximized true -> false
- close button: close-request received = true
- minimize button: toplevel state changes afterwards ["ToplevelState(TOP_RESIZABLE | RIGHT_RESIZABLE | BOTTOM_RESIZABLE | LEFT_RESIZABLE)"], suspended now = false

