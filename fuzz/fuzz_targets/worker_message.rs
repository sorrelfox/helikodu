// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Decodes arbitrary bytes as a message between the scanner and a scan worker, and
//! reads them as a stream of frames. Any panic or large allocation is a bug, and so
//! is a message that decodes but does not encode back to the same bytes.

#![no_main]

use helikodu_meta::worker::{Message, read_frame};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(message) = Message::decode(data) {
        let again = message.encode();
        match message {
            // A greeting of another version decodes only as far as its version.
            Message::Hello(_) => assert!(data.starts_with(&again[..again.len().min(7)])),
            _ => assert_eq!(again, data),
        }
    }
    let mut stream = data;
    while let Ok(Some(frame)) = read_frame(&mut stream) {
        let _ = Message::decode(&frame);
    }
});
