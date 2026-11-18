// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The desktop files in `data/`: they agree with the code, and pass
//! `desktop-file-validate` and `appstreamcli validate` where those are installed
//! (skipped with a note otherwise).

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use helikodu_desktop::APP_ID;

fn data(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../data")
        .join(name)
}

/// Runs `program` with `args`; `None` when it is not installed.
fn run(program: &str, args: &[&str]) -> Option<std::process::Output> {
    match Command::new(program).args(args).output() {
        Ok(output) => Some(output),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipped: {program} is not installed");
            None
        }
        Err(error) => panic!("{program}: {error}"),
    }
}

#[test]
fn the_desktop_file_is_valid_and_names_the_app() {
    let path = data(&format!("{APP_ID}.desktop"));
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains(&format!("Icon={APP_ID}\n")));
    assert!(text.contains("DBusActivatable=true\n"));
    assert!(text.contains("Actions=play-pause;next;previous;\n"));
    for action in ["play-pause", "next", "previous"] {
        assert!(text.contains(&format!("[Desktop Action {action}]")));
        assert!(text.contains(&format!("Exec=gapplication action {APP_ID} {action}\n")));
    }
    let mime_types = text
        .lines()
        .find_map(|line| line.strip_prefix("MimeType="))
        .unwrap();
    assert!(mime_types.split(';').any(|t| t == "audio/flac"));
    assert!(mime_types.ends_with(';'));

    if let Some(output) = run("desktop-file-validate", &[path.to_str().unwrap()]) {
        let messages = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{messages}");
        assert!(messages.trim().is_empty(), "{messages}");
    }
}

#[test]
fn the_metainfo_is_valid() {
    let path = data(&format!("{APP_ID}.metainfo.xml"));
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains(&format!("<id>{APP_ID}</id>")));
    assert!(text.contains("<metadata_license>CC0-1.0</metadata_license>"));
    assert!(text.contains("<project_license>GPL-3.0-or-later</project_license>"));
    assert!(text.contains(&format!(
        "<launchable type=\"desktop-id\">{APP_ID}.desktop</launchable>"
    )));

    if let Some(output) = run(
        "appstreamcli",
        &["validate", "--no-net", "--no-color", path.to_str().unwrap()],
    ) {
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn the_dbus_service_starts_the_app() {
    let text = fs::read_to_string(data(&format!("{APP_ID}.service.in"))).unwrap();
    assert!(text.contains("[D-BUS Service]\n"));
    assert!(text.contains(&format!("Name={APP_ID}\n")));
    assert!(text.contains("Exec=@bindir@/helikodu --gapplication-service\n"));
}
