// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What fake portals share: answering a request through its Request object,
//! as the real portal does, and the session path a client expects.

use std::collections::HashMap;

use zbus::message::Header;
use zbus::names::BusName;
use zbus::{Connection, OwnedVariant, Variant};

/// The object path of the portal frontend.
pub(crate) const DESKTOP_PATH: &str = "/org/freedesktop/portal/desktop";

/// The path of a request or session object (`kind` is `request` or `session`)
/// of the caller of `header`, with `token`.
pub(crate) fn handle_path(header: &Header<'_>, kind: &str, token: &str) -> String {
    let sender = header
        .sender()
        .map(|s| s.trim_start_matches(':').replace('.', "_"))
        .unwrap_or_default();
    format!("{DESKTOP_PATH}/{kind}/{sender}/{token}")
}

/// A string option of a call.
pub(crate) fn option(options: &HashMap<String, OwnedVariant>, key: &str) -> String {
    options
        .get(key)
        .and_then(|v| v.downcast_ref::<&str>().ok().map(str::to_owned))
        .unwrap_or_default()
}

/// Answers the request named by the `handle_token` option with `results`.
pub(crate) async fn respond(
    connection: &Connection,
    header: &Header<'_>,
    options: &HashMap<String, OwnedVariant>,
    results: HashMap<&str, Variant<'_>>,
) {
    let path = handle_path(header, "request", &option(options, "handle_token"));
    connection
        .emit_signal(
            header.sender().map(|s| BusName::from(s.to_owned())),
            path.as_str(),
            "org.freedesktop.portal.Request",
            "Response",
            &(0u32, results),
        )
        .await
        .expect("send the response");
}

/// The results of a `CreateSession` or `CreateMonitor` call: the session's
/// path, as a string, as the portal sends it.
pub(crate) fn session_results(
    header: &Header<'_>,
    options: &HashMap<String, OwnedVariant>,
) -> (String, HashMap<&'static str, Variant<'static>>) {
    let session = handle_path(header, "session", &option(options, "session_handle_token"));
    let mut results = HashMap::new();
    results.insert("session_handle", Variant::from(session.clone()));
    (session, results)
}
