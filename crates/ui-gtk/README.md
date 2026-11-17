# helikodu-ui-gtk

The desktop user interface, built with GTK 4 without libadwaita. It talks to the core only through `service`, never to `engine` or `library` directly, so another front end could be added without touching the core.

- **May depend on:** `model`, `service`, `desktop`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

## Tests

```sh
cargo test -p helikodu-ui-gtk
```
