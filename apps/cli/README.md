# helikodu-cli

Builds `helikodu-cli`: remote control of the running application over D-Bus, and offline tools (scan, info, verify, convert, list-outputs) that use the core directly. It turns arguments into calls on the core and prints the results.

- **May depend on:** `model`, `service`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

## Tests

```sh
cargo test -p helikodu-cli
```
