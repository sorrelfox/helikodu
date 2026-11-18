// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Links libvgmstream when the `link` feature asks for it.
//!
//! The library is found through pkg-config (`libvgmstream`), or in the folder named
//! by `VGMSTREAM_LIB_DIR`. When it is not found the build goes on with stub functions
//! that fail, and prints a warning; with `HELIKODU_REQUIRE_VGMSTREAM=1` it fails
//! instead (for packages that must ship the decoder).

use std::env;
use std::path::Path;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(vgmstream_linked)");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    println!("cargo:rerun-if-env-changed=VGMSTREAM_LIB_DIR");
    println!("cargo:rerun-if-env-changed=HELIKODU_REQUIRE_VGMSTREAM");
    if env::var_os("CARGO_FEATURE_LINK").is_none() {
        return;
    }
    let found = match pkg_config::Config::new()
        .atleast_version("1.1")
        .probe("libvgmstream")
    {
        Ok(_) => Ok(()),
        Err(error) => match env::var_os("VGMSTREAM_LIB_DIR") {
            Some(dir) if Path::new(&dir).is_dir() => {
                println!(
                    "cargo:rustc-link-search=native={}",
                    Path::new(&dir).display()
                );
                println!("cargo:rustc-link-lib=vgmstream");
                Ok(())
            }
            _ => Err(error),
        },
    };
    match found {
        Ok(()) => {
            println!("cargo:rustc-cfg=vgmstream_linked");
            println!("cargo:linked=1");
        }
        Err(error) => {
            let message = format!(
                "libvgmstream (public API 1.1 or newer) was not found through pkg-config \
                 ({error}) nor in VGMSTREAM_LIB_DIR. Most distributions do not package \
                 vgmstream: build it from its sources with CMake (BUILD_SHARED_LIBS=ON) and \
                 point PKG_CONFIG_PATH or VGMSTREAM_LIB_DIR at it. The vgmstream decoder is \
                 left out of this build."
            );
            if env::var_os("HELIKODU_REQUIRE_VGMSTREAM").is_some_and(|v| v == "1") {
                panic!("\n\n{message}\n\n");
            }
            println!("cargo:warning={message}");
        }
    }
}
