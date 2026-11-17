# crates

Every library of the workspace, one folder per crate, not nested. The package name is `helikodu-<folder>`.

| Crate | Responsibility |
|---|---|
| `model` | Shared types: identifiers, audio specifications, library records, settings, commands and events. No I/O |
| `ffmpeg-bridge` | Safe wrapper over the FFmpeg C API, restricted to local files |
| `openmpt-sys`, `gme-sys`, `vgmstream-sys`, `sidplayfp-sys` | Raw bindings, one crate per C or C++ library. No logic |
| `extra-decoders` | Safe decoders over those bindings, the MIDI synthesizer and the DSF/DFF reader |
| `engine` | Decoding, processing, output backends and the transport |
| `meta` | Tags, covers, CUE sheets, lyrics, playlist formats and naming templates |
| `library` | Database, scanner, watcher, search, query language and statistics |
| `service` | The application core shared by every front end |
| `desktop` | MPRIS2, the D-Bus API, notifications, portals and the tray icon |
| `ui-gtk` | The GTK 4 front end |
| `testkit` | Shared test helpers; a development dependency only |

## Dependency rules

```text
model ─┬─> ffmpeg-bridge ─┬─> engine ──┐
       │                  │            ├─> service ─┬─> desktop ─┐
       ├─> extra-decoders ┘            │            │            ├─> ui-gtk
       │   (+ the -sys crates)         │            └────────────┘
       └─> meta ─> library ────────────┘
```

- `ui-gtk` never uses `engine` or `library` directly; everything goes through `service`.
- `engine` knows nothing about the library; it plays media sources.
- Only `ffmpeg-bridge`, `extra-decoders` and the `-sys` crates may contain `unsafe` code. Every other crate has `#![forbid(unsafe_code)]`.
- `testkit` is only ever a development dependency.
- No crate may depend on network, TLS or DNS crates. Helikodu works entirely offline.

`cargo xtask check-deps` and `cargo deny check` enforce these rules in CI.

## Inside a crate

```text
crates/<name>/
├── Cargo.toml     takes version, edition, licence, lints and dependency versions from the workspace
├── README.md      what the crate is for, what it may depend on, how to run its tests
├── src/
│   ├── lib.rs     the public API and the list of modules, nothing else
│   └── <area>/    one folder per responsibility
├── tests/         tests through the public API
├── benches/       benchmarks
└── examples/      small runnable programs
```

- A module is one file. When it needs parts of its own, it becomes a folder beside a file of the same name (`decode.rs` and `decode/`). There are no `mod.rs` files.
- Items are private or `pub(crate)` unless another crate needs them.
- Unit tests sit at the bottom of the file they test; larger tests go in `tests/`.
- Data compiled into a crate has a fixed place: `migrations/` in `library`, `resources/` in `ui-gtk`, `assets/` anywhere else.
- No module is called `utils`, `common`, `helpers` or `misc`. Code is named after what it does.
