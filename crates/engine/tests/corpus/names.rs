// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The names set: files whose names are not valid UTF-8 (Latin-1, cp1251 and
//! Shift-JIS bytes, in a folder whose name is not UTF-8 either), names of 255
//! bytes, with a newline, a leading dash, spaces, characters Windows forbids, and
//! NFC and NFD forms. Each plays through the transport, bit-exact.

use helikodu_model::player::{EngineEvent, TrackEndReason, TransportState};

use crate::decoding;
use crate::manifest::Corpus;
use crate::media;
use crate::report::Table;
use crate::session::{Session, item, wav_config};

#[test]
fn files_with_any_name_play() {
    let Some(corpus) = Corpus::load("names") else {
        return;
    };
    let mut table = Table::new("names", &["file", "UTF-8", "bytes", "frames"]);
    for (i, entry) in corpus.set("names").iter().enumerate() {
        let path = corpus.path(entry);
        let bytes = entry.path_bytes();
        let utf8 = std::str::from_utf8(&bytes).is_ok();
        let name_len = bytes.rsplit(|b| *b == b'/').next().map_or(0, <[u8]>::len);
        let reference = decoding::decode(&path);
        let out = media::path(&format!("corpus-names-{i}.wav"));
        let mut session = Session::new(wav_config(&out));
        session.play_queue(&[item(1, &path)]);
        session.wait_state(TransportState::Stopped);
        let finished = session.log.contains(&EngineEvent::TrackEnded {
            item: helikodu_model::player::ItemId::new(1),
            reason: TrackEndReason::Finished,
        });
        drop(session);
        let mut cells = vec![
            entry.display(),
            if utf8 { "yes" } else { "no" }.into(),
            name_len.to_string(),
        ];
        let problem = match (finished, reference) {
            (false, _) => {
                cells.push("-".into());
                Some("did not play to its end".to_owned())
            }
            (true, Err(error)) => {
                cells.push("-".into());
                Some(format!("the registry cannot open it: {error}"))
            }
            (true, Ok(reference)) => {
                let written = media::read_output(&out);
                cells.push(written.frames().to_string());
                let got: Vec<f32> = written
                    .ints
                    .iter()
                    .map(|&v| f32::from(v) / 32_768.0)
                    .collect();
                (got != reference.samples).then(|| "the output differs from the file".to_owned())
            }
        };
        table.row(cells, problem);
    }
    table.finish();
}
