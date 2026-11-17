# helikodu

Builds `helikodu`, the desktop application. It only wires the parts together: it starts the service, hands it the desktop integrations it needs and runs the GTK front end. It holds no domain logic.

- **May depend on:** `model`, `service`, `desktop`, `ui-gtk`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

## Tests

```sh
cargo test -p helikodu
```
