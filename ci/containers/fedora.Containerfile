# SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
# SPDX-License-Identifier: GPL-3.0-or-later

# Build and test environment: current Fedora, with the full FFmpeg build
# from RPM Fusion instead of the restricted ffmpeg-free.
FROM registry.fedoraproject.org/fedora:latest

RUN dnf install -y \
      "https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-$(rpm -E %fedora).noarch.rpm" \
    && dnf install -y --allowerasing \
      alsa-lib-devel ca-certificates clang clang-devel curl gcc git gtk4-devel meson ninja-build \
      pipewire-devel pkgconf-pkg-config sqlite-devel ffmpeg-devel \
    && dnf clean all

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
