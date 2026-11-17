# apps

One folder per installed program. The programs hold no domain logic: they parse arguments, wire the libraries in `crates/` together and, in the CLI, print results. Everything else lives in a library, where it can be tested.

| Folder | Program | Purpose |
|---|---|---|
| `gui/` | `helikodu` | The desktop application |
| `cli/` | `helikodu-cli` | Remote control of the running application, and offline tools |
| `scan-worker/` | `helikodu-scan-worker` | Isolated metadata extraction, started by the library scanner |

Optional Cargo features are declared in the crate that owns the dependency and passed on by name here, because these are the packages that distributions build.
