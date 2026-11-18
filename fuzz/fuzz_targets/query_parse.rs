// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Parses arbitrary text as a query of the query language. Any panic is a bug, and so
//! is an error whose span is not a range of whole characters inside the text. What
//! parses must print to text that parses to the same tree, and the lenient parser
//! must always give a query whose comparisons fit together.

#![no_main]

use helikodu_library::{parse_query, parse_query_lenient, print_query};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    match parse_query(text) {
        Ok(query) => {
            assert_eq!(query.problem(), None);
            let printed = print_query(&query);
            match parse_query(&printed) {
                Ok(again) => assert_eq!(again, query, "{printed:?}"),
                Err(error) => panic!("{printed:?} does not read back: {error}"),
            }
        }
        Err(error) => {
            let span = error.span();
            assert!(span.start <= span.end && span.end <= text.len());
            assert!(text.is_char_boundary(span.start) && text.is_char_boundary(span.end));
            let _ = (error.to_string(), error.example());
        }
    }
    let lenient = parse_query_lenient(text);
    assert_eq!(lenient.query.problem(), None);
    let _ = print_query(&lenient.query);
});
