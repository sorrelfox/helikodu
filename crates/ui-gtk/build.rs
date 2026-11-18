// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Compiles `resources/` (stylesheet, templates, icons) into the resource
//! bundle that the crate includes.

#![forbid(unsafe_code)]

fn main() {
    glib_build_tools::compile_resources(
        &["resources"],
        "resources/helikodu.gresource.xml",
        "helikodu.gresource",
    );
}
