# System tests

Tests that run the built programs: the command-line tool, the D-Bus API and MPRIS, the offline audit (no network socket is ever opened), robustness and soak tests.

| Folder | Content |
|---|---|
| `offline-launcher/` | The seccomp launcher used by the offline audit; see its [README](offline-launcher/README.md) |

## Offline audit

Helikodu never opens a network connection. `cargo xtask offline-audit` checks that the test suite does not either:

1. It builds `helikodu-offline-launcher` and every test program of the workspace (`cargo test --workspace --all-features --no-run`).
2. It runs a self-test: in a new network namespace only the loopback device exists; an `AF_INET` socket probe survives without the filter, and dies with `SIGSYS` under it; an `AF_INET6` probe dies too; `AF_UNIX` sockets still work under the filter. If any of these fails, the audit stops.
3. It runs every test program as

   ```sh
   unshare --user --net --map-current-user -- helikodu-offline-launcher -- <test program>
   ```

   from the program's package folder, as `cargo test` does. The network namespace has no interfaces but a loopback device that is down; the launcher's seccomp filter kills the program with `SIGSYS` the moment it calls `socket()` or `socketpair()` with `AF_INET` or `AF_INET6`. `AF_UNIX` (D-Bus, PipeWire) and `AF_NETLINK` sockets are not affected.
4. A test program that fails or is killed fails the audit, which names it and says why. `killed by SIGSYS` means the program tried to create an Internet socket.

`cargo xtask offline-audit --self-test` runs only steps 1 (the launcher) and 2. Arguments after `--` go to every test program, for example `cargo xtask offline-audit -- --test-threads=1`.

To find the test that opened the socket, run the named program under the launcher by hand and narrow it down with a test-name filter, or look at the core dump (`coredumpctl gdb`): `SIGSYS` from seccomp stops the process at the offending system call.

Documentation tests are not part of the audit: Cargo does not build them as separate programs.

### Requirements

- Linux on x86-64, AArch64 or RISC-V 64.
- `unshare` from util-linux. `--map-current-user` (util-linux 2.38 or newer) keeps your user ID inside the namespace; with older releases the audit falls back to `--map-root-user` and says so.
- Unprivileged user namespaces. They are on by default on Fedora, Debian, Arch and most other distributions. On Ubuntu 23.10 and newer, AppArmor restricts them; allow them with `sudo sysctl kernel.apparmor_restrict_unprivileged_userns=0`.

`cargo xtask ci` runs the audit as its last step. Where the namespace cannot be created it is skipped with a notice, except in CI (when `CI` is set), where it is an error. `cargo xtask ci --no-offline-audit` leaves it out.

### In containers

Docker's default seccomp profile refuses to create user namespaces (`unshare failed: Operation not permitted`), and on AppArmor hosts its default AppArmor profile may as well. Podman's default profile allows it. Run a Docker container with

```sh
docker run --security-opt seccomp=unconfined --security-opt apparmor=unconfined ...
```

On a host whose AppArmor restricts unprivileged user namespaces (GitHub's Ubuntu runners), set `kernel.apparmor_restrict_unprivileged_userns=0` on the host first. The `offline-audit` job in `.github/workflows/ci.yml` does both. No extra capabilities and no `--privileged` are needed: the namespace is made without privileges inside the container.

### What it does not cover

- Sockets created without the `socket()` system call, for example through `io_uring`, are not caught by the filter. The namespace still gives them nothing to reach: it is the second, independent line of defence.
- Programs started by the tests inherit both the namespace and the filter, so they are covered.
