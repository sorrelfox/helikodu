// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The worker's side: answer requests until the scanner closes the pipe.

use std::io::{self, Read, Write};

use super::message::{Hello, Message, Request, Response};
use super::wire::{read_frame, write_frame};

/// Serves requests from `input` and writes the responses to `output`, one at a
/// time, with `handler` (normally [`handle`](super::handle)).
///
/// Says [`Hello`] first. Returns `Ok` when `input` ends between frames, which is how
/// the scanner stops a worker, and an error when a frame cannot be read or decoded or
/// a response cannot be written; the worker should then exit, and the scanner starts
/// another.
pub fn serve(
    mut input: impl Read,
    mut output: impl Write,
    mut handler: impl FnMut(&Request) -> Response,
) -> io::Result<()> {
    write_frame(&mut output, &Message::Hello(Hello::current()).encode())?;
    while let Some(frame) = read_frame(&mut input)? {
        let request = match Message::decode(&frame) {
            Ok(Message::Request(request)) => request,
            Ok(other) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("expected a request, got {other:?}"),
                ));
            }
            Err(e) => return Err(io::Error::new(io::ErrorKind::InvalidData, e)),
        };
        let response = handler(&request);
        write_frame(&mut output, &Message::Response(Box::new(response)).encode())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::message::{Failure, FailureKind, RequestOptions};

    #[test]
    fn answers_every_request_in_order() {
        let mut input = Vec::new();
        for id in [3, 1, 2] {
            let request = Request {
                id,
                path: format!("/x/{id}").into(),
                options: RequestOptions::default(),
            };
            write_frame(&mut input, &Message::Request(request).encode()).unwrap();
        }
        let mut output = Vec::new();
        serve(input.as_slice(), &mut output, |request| Response {
            id: request.id,
            result: Err(Failure {
                kind: FailureKind::Io,
                message: request.path.display().to_string(),
            }),
        })
        .unwrap();

        let mut stream = output.as_slice();
        let mut messages = Vec::new();
        while let Some(frame) = read_frame(&mut stream).unwrap() {
            messages.push(Message::decode(&frame).unwrap());
        }
        assert_eq!(messages[0], Message::Hello(Hello::current()));
        let ids: Vec<u64> = messages[1..]
            .iter()
            .map(|m| match m {
                Message::Response(r) => r.id,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(ids, [3, 1, 2]);
    }

    #[test]
    fn garbage_ends_the_loop_with_an_error() {
        let mut input = Vec::new();
        write_frame(&mut input, b"\x09garbage").unwrap();
        let result = serve(input.as_slice(), io::sink(), |_| unreachable!());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
}
