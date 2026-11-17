# SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
# SPDX-License-Identifier: GPL-3.0-or-later

# Build and test environment: Arch Linux (rolling, latest FFmpeg).
FROM docker.io/library/archlinux:latest

RUN pacman -Syu --noconfirm --needed \
      alsa-lib base-devel ca-certificates clang curl ffmpeg git gtk4 libpipewire meson ninja \
      pkgconf sqlite \
    && pacman -Scc --noconfirm

# Rust comes from rustup without a default toolchain: the toolchain pinned in
# rust-toolchain.toml is installed when the repository is mounted.
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --no-modify-path --profile minimal --default-toolchain none \
    && chmod -R a+rwX /usr/local/rustup /usr/local/cargo

WORKDIR /src
