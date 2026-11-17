# helikodu-offline-launcher

The launcher behind `cargo xtask offline-audit`. It installs a seccomp filter on itself and then replaces itself with the program it was given:

```sh
helikodu-offline-launcher [--] <program> [<argument>...]
```

The filter kills the program, and everything it starts, with `SIGSYS` as soon as it calls `socket()` or `socketpair()` with the domain `AF_INET` or `AF_INET6`. Every other system call is allowed, so `AF_UNIX` sockets (D-Bus, PipeWire) and `AF_NETLINK` sockets keep working. The filter cannot be removed once installed.

`--probe inet|inet6|unix|interfaces` makes one socket call (or lists the network interfaces) and reports what happened. The audit's self-test runs the probes with and without the filter to prove that the harness catches Internet sockets and leaves `AF_UNIX` alone.

## Why a separate program

Installing a filter in a child process from `xtask` would need a `pre_exec` hook, which is `unsafe`, and `xtask` forbids `unsafe` code. This program installs the filter on itself with the safe `seccompiler::apply_filter` and then calls `exec`, so it also forbids `unsafe` code. It is a workspace member so that it shares the lock file, the lints and the dependency checks; nothing depends on it.

## Supported systems

Linux on x86-64, AArch64 and RISC-V 64 (the architectures `seccompiler` supports). The filter checks the system-call architecture first and kills a process that uses another architecture's interface; on x86-64 the x32 numbers of both calls are covered as well.

## Tests

`cargo test -p helikodu-offline-launcher` runs the probes under the filter without a namespace, so the tests run wherever the test suite runs, including inside the audit itself.
