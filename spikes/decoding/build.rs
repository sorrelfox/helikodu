// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

fn main() {
    // Export the avcodec_open2 wrapper (src/probe.rs) from the executable so
    // that libavformat's internal calls bind to it as well.
    println!("cargo:rustc-link-arg-bins=-Wl,--export-dynamic-symbol=avcodec_open2");
}
