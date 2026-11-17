# helikodu-scan-worker

Builds `helikodu-scan-worker`, which reads the tags, audio properties, quick hash, CUE sheet tracks and chapters of one file at a time over stdin and stdout. The scanner runs a pool of these processes with time and memory limits, so a decoder that crashes or hangs on a broken file cannot take the application down.

- **May depend on:** `model`, `meta`, `ffmpeg-bridge`, `extra-decoders`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

The program is wiring only. The protocol and the reading are in `helikodu_meta::worker`; the pool that starts and stops the workers is in `helikodu-library` (`src/scan/pool.rs`).

## Running

```text
helikodu-scan-worker [--memory-limit BYTES]
```

Before it reads anything, the worker:

- limits its address space to `--memory-limit` bytes (1 GiB by default; 0 for no limit), soft and hard, with `setrlimit` through the `rlimit` crate;
- sets the core-dump size to 0, so a crash leaves no core file;
- sets FFmpeg's log level to quiet.

It then says hello (the protocol version) and answers requests until its input closes, when it exits with status 0. A malformed request ends it with status 1, and a bad argument with status 2. A reader that panics is answered as a failure and the worker carries on; a reader that crashes the process or exceeds the memory limit kills it, and the scanner starts another.

## Deliberate failures for tests

A debug build honours `HELIKODU_SCAN_WORKER_FAULTS=1`: on a file whose name contains `crash-worker` it aborts, on `hang-worker` it never answers, and on `hog-worker` it allocates 4 GiB, which the address-space limit refuses. A release build ignores the variable. The scanner tests in `helikodu-library` use these markers.

## Tests

```sh
cargo test -p helikodu-scan-worker
```

`tests/protocol.rs` runs the program over its pipes: the greeting, answers for a missing and an unreadable file, a clean exit when the input closes, the end on garbage input, a bad argument, and (in debug builds) the address-space limit stopping a runaway allocation.
