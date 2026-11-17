# xtask

Developer commands, run as `cargo xtask <command>`. They are written in Rust rather than shell so that they work the same on every machine and in CI.

| Command | What it does |
|---|---|
| `ci` | Runs every check CI runs, in the same order: formatting, clippy, tests, the dependency rules, `cargo deny` and the REUSE lint |
| `check-deps` | Checks that each crate depends only on the workspace crates it may use, that `helikodu-testkit` is only a development dependency, that every crate outside the FFI bridges and binding crates forbids `unsafe` code, and that every crate has a README |

More commands arrive as the work needs them: `corpus` (generate the test corpus), `synth-library` (synthetic libraries for benchmarks), `codegen` (code generated from the design tokens), `dist` (release tarballs) and `man` (man pages).

`xtask` never links the product crates. When it needs them, it runs Cargo or the built programs.
