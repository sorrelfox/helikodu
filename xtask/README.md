# xtask

Developer commands, run as `cargo xtask <command>`. They are written in Rust rather than shell so that they work the same on every machine and in CI.

| Command | What it does |
|---|---|
| `ci` | Runs every check CI runs, in the same order: formatting, clippy, tests, the dependency rules, `cargo deny`, the REUSE lint and the offline audit. `--tests-only` builds, tests and checks the dependency rules without the lint tools. The offline audit is skipped with a notice where unprivileged user namespaces are not available (an error in CI); `--no-offline-audit` leaves it out |
| `check-deps` | Checks that each crate depends only on the workspace crates it may use, that `helikodu-testkit` is only a development dependency, that every crate outside the FFI bridges and binding crates forbids `unsafe` code, and that every crate has a README |
| `corpus` | Generates the test corpus from the recipes in `tests/corpus/` into `target/corpus/`, with a manifest of SHA-256 checksums and tool versions. Files that are up to date are kept. A recipe whose program or FFmpeg encoder is missing is skipped with a notice; `--strict` makes that an error. `--check` verifies an existing corpus against its manifest. Uses FFmpeg, `ffprobe` to verify each file and, where installed, `flac`, `lame`, `opusenc`, `wavpack`, `sox`, `oggenc`, `mac` and `mpcenc`. See [tests/corpus/README.md](../tests/corpus/README.md) |
| `offline-audit` | Runs every test program of the workspace inside a new network namespace with no interfaces (`unshare --user --net`), under a seccomp filter that kills the program with `SIGSYS` if it creates an `AF_INET` or `AF_INET6` socket; `AF_UNIX` and `AF_NETLINK` sockets keep working. A program that fails or is killed fails the audit, which names it. A self-test first proves that the harness catches Internet sockets; `--self-test` runs only that. Arguments after `--` go to every test program. The filter is installed by `helikodu-offline-launcher`, so `xtask` needs no `unsafe` code. See [tests/system/README.md](../tests/system/README.md) |

More commands arrive as the work needs them: `synth-library` (synthetic libraries for benchmarks), `codegen` (code generated from the design tokens), `dist` (release tarballs) and `man` (man pages).

`xtask` never links the product crates. When it needs them, it runs Cargo or the built programs.
