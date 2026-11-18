# ci

Container definitions for the build matrix, and the headless PipeWire, ALSA and seccomp configurations the tests use.

The workflows in `.github/` only start `cargo xtask ci` inside these containers, so every CI check can also be run locally. They run only for release tags and when started by hand from the Actions tab; day-to-day changes are checked locally.

## Containers

`ci/containers/` has one Containerfile per distribution in the build matrix:

| File | Base image | Notes |
|---|---|---|
| `debian.Containerfile` | `debian:13` | Also used for the minimum supported Rust version build |
| `ubuntu.Containerfile` | `ubuntu:26.04` | |
| `fedora.Containerfile` | `fedora:latest` | Full FFmpeg from RPM Fusion; also runs the lint job |
| `arch.Containerfile` | `archlinux:latest` | Latest FFmpeg |

Each image contains a C toolchain, pkg-config, clang and libclang (for bindgen), Meson and Ninja, and the development packages for GTK 4, FFmpeg, PipeWire, ALSA, JACK (`jack.pc`, needed to build the JACK output backend although `libjack` is loaded at run time) and SQLite; the PulseAudio backend needs none, as it speaks the protocol itself. Rust is installed with rustup but without a toolchain; `rustup toolchain install` in the mounted repository installs the version pinned in `rust-toolchain.toml`.

To run the checks locally in a container:

```sh
podman build -t helikodu-ci:fedora -f ci/containers/fedora.Containerfile ci/containers
podman run --rm -it -e CI=true -v "$PWD:/src:Z" helikodu-ci:fedora \
    sh -c 'rustup toolchain install && cargo xtask ci'
```

The lint job also installs `reuse` and `cargo-deny`, which `cargo xtask ci` requires when `CI` is set.

## Architectures

The checks run on x86_64. The Release workflow also builds and tests on aarch64, natively on GitHub's arm64 runner (`ubuntu-24.04-arm`, free for public repositories): Docker there pulls the arm64 variant of `debian:13`, and the same Containerfile, memory limit and job count apply. To try the aarch64 build locally on an x86_64 machine, Podman can emulate it with `qemu-user-static` installed (`podman build --platform linux/arm64 …`), which is slow but needs no changes.

## Offline audit

The `offline-audit` job runs `cargo xtask offline-audit` in the Fedora container. The audit makes an unprivileged user and network namespace inside the container, which Docker's default seccomp profile refuses and which Ubuntu runners restrict through AppArmor. The job therefore sets `kernel.apparmor_restrict_unprivileged_userns=0` on the runner and starts the container with `--security-opt seccomp=unconfined --security-opt apparmor=unconfined`. It adds no capabilities and does not use `--privileged`. The container needs `unshare` from util-linux, which every image here has. The lint job runs `cargo xtask ci --no-offline-audit`, since the audit has its own job. Details are in [tests/system/README.md](../tests/system/README.md#offline-audit).

To run the audit locally in a container (Podman's default profiles allow it):

```sh
podman run --rm -it -e CI=true -v "$PWD:/src:Z" helikodu-ci:fedora \
    sh -c 'rustup toolchain install && cargo xtask offline-audit'
```

## Memory limit

A GitHub-hosted runner stops a job whose build uses more than 12 GB of memory. The workflows keep every job well under it:

- `CARGO_BUILD_JOBS: 4` is set in the `env` of each workflow and passed into the containers (`-e CARGO_BUILD_JOBS`), so Cargo never runs more than four compiler or linker processes at once, whatever number of CPUs the runner reports. Meson's release build inherits it, as `meson compile` runs Cargo with the caller's environment.
- Every `docker run` has `--memory 12g --memory-swap 12g`. A job that outgrows the limit is then stopped inside its own step by the kernel's out-of-memory killer, with a visible error, instead of taking the runner down.

The release profile (`lto = "thin"`, `codegen-units = 1`) is the same in CI as for the published binaries; it does not need a CI override.

Measured in `helikodu-ci:debian` with `--cpus 4` and an empty target folder (cgroup `memory.peak`, which counts the page cache of the files written as well as the memory of the processes):

| Step | Peak, no limit | Process memory (anon) peak | With `--memory 10g` |
|---|---|---|---|
| `cargo build --release --workspace` (the Meson release build is the same compilation) | 4.1 GB | | 4.2 GB, no OOM kill |
| `cargo test --workspace --all-features` (building the tests) | 12.5 GB | 2.2 GB | 10 GB (the cap; cache reclaimed), no OOM kill |
| `cargo clippy --workspace --all-targets --all-features` | 3.6 GB | | 3.2 GB, no OOM kill |

Most of the test build's peak is page cache for the debug artifacts it writes, which the kernel reclaims under a limit rather than counting against the job. To check a change that might raise memory use, run the step in a container with a hard limit and look for `oom_kill` in the container's `memory.events`:

```sh
podman run --rm --cpus 4 --memory 10g --memory-swap 10g -e CARGO_BUILD_JOBS=4 \
    -v "$PWD:/src:Z" helikodu-ci:debian sh -c 'rustup toolchain install &&
    cargo test --workspace --all-features --no-run;
    cat /sys/fs/cgroup/memory.peak /sys/fs/cgroup/memory.events'
```
