# SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
# SPDX-License-Identifier: GPL-3.0-or-later

# Build and test environment: ubuntu (ubuntu:26.04).
FROM docker.io/library/ubuntu:26.04

ENV DEBIAN_FRONTEND=noninteractive
# The long-tail decoders: libopenmpt, libgme and libsidplayfp (the SID shim is C++).
# vgmstream is not packaged; its decoder is off by default and not built here.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
    build-essential ca-certificates clang curl git libclang-dev meson ninja-build pkg-config \
    libasound2-dev libavcodec-dev libavfilter-dev libavformat-dev libavutil-dev libswresample-dev \
    libgtk-4-dev libjack-jackd2-dev libpipewire-0.3-dev libsqlite3-dev \
    libgme-dev libopenmpt-dev libsidplayfp-dev \
    && rm -rf /var/lib/apt/lists/*

# Rust comes from rustup without a default toolchain: the toolchain pinned in
# rust-toolchain.toml is installed when the repository is mounted.
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --no-modify-path --profile minimal --default-toolchain none \
    && chmod -R a+rwX /usr/local/rustup /usr/local/cargo

WORKDIR /src

# The checkout is mounted from the host and owned by another user; let git
# (and tools that ask git, such as reuse) work in it.
RUN git config --system --add safe.directory "*"
