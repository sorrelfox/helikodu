# ci

Container definitions for the build matrix, and the headless PipeWire, ALSA and seccomp configurations the tests use.

The workflows in `.github/` only start `cargo xtask ci` inside these containers, so every CI check can also be run locally.

## Containers

`ci/containers/` has one Containerfile per distribution in the build matrix:

| File | Base image | Notes |
|---|---|---|
| `debian.Containerfile` | `debian:13` | Also used for the minimum supported Rust version build |
| `ubuntu.Containerfile` | `ubuntu:26.04` | |
| `fedora.Containerfile` | `fedora:latest` | Full FFmpeg from RPM Fusion; also runs the lint job |
| `arch.Containerfile` | `archlinux:latest` | Latest FFmpeg |

Each image contains a C toolchain, pkg-config, clang and libclang (for bindgen), Meson and Ninja, and the development packages for GTK 4, FFmpeg, PipeWire, ALSA and SQLite. Rust is installed with rustup but without a toolchain; `rustup toolchain install` in the mounted repository installs the version pinned in `rust-toolchain.toml`.

To run the checks locally in a container:

```sh
podman build -t helikodu-ci:fedora -f ci/containers/fedora.Containerfile ci/containers
podman run --rm -it -e CI=true -v "$PWD:/src:Z" helikodu-ci:fedora \
    sh -c 'rustup toolchain install && cargo xtask ci'
```

The lint job also installs `reuse` and `cargo-deny`, which `cargo xtask ci` requires when `CI` is set.

## Offline audit

The `offline-audit` job runs `cargo xtask offline-audit` in the Fedora container. The audit makes an unprivileged user and network namespace inside the container, which Docker's default seccomp profile refuses and which Ubuntu runners restrict through AppArmor. The job therefore sets `kernel.apparmor_restrict_unprivileged_userns=0` on the runner and starts the container with `--security-opt seccomp=unconfined --security-opt apparmor=unconfined`. It adds no capabilities and does not use `--privileged`. The container needs `unshare` from util-linux, which every image here has. The lint job runs `cargo xtask ci --no-offline-audit`, since the audit has its own job. Details are in [tests/system/README.md](../tests/system/README.md#offline-audit).

To run the audit locally in a container (Podman's default profiles allow it):

```sh
podman run --rm -it -e CI=true -v "$PWD:/src:Z" helikodu-ci:fedora \
    sh -c 'rustup toolchain install && cargo xtask offline-audit'
```
