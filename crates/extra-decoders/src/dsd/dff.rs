// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The DSDIFF header (DFF, version 1.5). Big-endian.
//!
//! A `FRM8` form of type `DSD ` holding chunks, each padded to an even length:
//! `FVER`, a `PROP` chunk of type `SND ` (with the sample rate `FS  `, the channels
//! `CHNL` and the compression `CMPR`), and the sound data in a `DSD ` chunk, one
//! byte of each channel in turn. DST-compressed files have a `DST ` chunk instead.

use std::fs::File;

use helikodu_model::audio::{ChannelLayout, ChannelPosition};

use super::error::DsdError;
use super::reader::{DsdContainer, DsdInfo, Parsed, Storage, read_array};

/// The most chunks looked at before giving up on a file.
const MAX_CHUNKS: usize = 1024;

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&bytes[at..at + 8]);
    u64::from_be_bytes(b)
}

/// A chunk: its identifier, where its data starts and how long it is.
#[derive(Debug, Clone, Copy)]
struct Chunk {
    id: [u8; 4],
    data: u64,
    size: u64,
}

/// The chunks between `start` and `end`.
fn chunks(file: &File, start: u64, end: u64) -> impl Iterator<Item = Result<Chunk, DsdError>> {
    let mut at = start;
    let mut count = 0;
    std::iter::from_fn(move || {
        if at.checked_add(12)? > end || count >= MAX_CHUNKS {
            return None;
        }
        count += 1;
        let head: [u8; 12] = match read_array(file, at, "chunk header") {
            Ok(head) => head,
            Err(e) => {
                at = end;
                return Some(Err(e));
            }
        };
        let chunk = Chunk {
            id: [head[0], head[1], head[2], head[3]],
            data: at + 12,
            size: u64_at(&head, 4),
        };
        // Chunks are padded to an even length.
        at = chunk
            .data
            .saturating_add(chunk.size)
            .saturating_add(chunk.size & 1);
        Some(Ok(chunk))
    })
}

/// The position of a DSDIFF channel identifier.
fn position_of(id: &[u8; 4]) -> Option<ChannelPosition> {
    Some(match id {
        b"SLFT" | b"MLFT" => ChannelPosition::FrontLeft,
        b"SRGT" | b"MRGT" => ChannelPosition::FrontRight,
        b"C   " => ChannelPosition::FrontCenter,
        b"LFE " => ChannelPosition::LowFrequency,
        b"LS  " => ChannelPosition::BackLeft,
        b"RS  " => ChannelPosition::BackRight,
        _ => return None,
    })
}

/// The layout of the file's channels and, for each channel in WAVE order, its index
/// in the file. Unknown or repeated identifiers keep the file's order.
fn layout_of(ids: &[[u8; 4]]) -> Option<(ChannelLayout, Vec<usize>)> {
    let count = u16::try_from(ids.len()).ok()?;
    let positions: Option<Vec<_>> = ids.iter().map(position_of).collect();
    if let Some(positions) = positions
        && let Some(layout) = ChannelLayout::from_positions(positions.iter().copied())
    {
        let mut order = vec![0; ids.len()];
        for (file_index, position) in positions.into_iter().enumerate() {
            order[layout.index_of(position)?] = file_index;
        }
        return Some((layout, order));
    }
    let layout = ChannelLayout::default_for_count(count)?;
    Some((layout, (0..ids.len()).collect()))
}

/// The sample rate, channels and compression from the `PROP` chunk.
#[derive(Debug, Default)]
struct Properties {
    rate: Option<u32>,
    channels: Option<Vec<[u8; 4]>>,
}

fn properties(file: &File, prop: Chunk) -> Result<Properties, DsdError> {
    let kind: [u8; 4] = read_array(file, prop.data, "PROP chunk")?;
    if &kind != b"SND " {
        return Err(DsdError::malformed("the PROP chunk is not of type SND"));
    }
    let mut props = Properties::default();
    for chunk in chunks(file, prop.data + 4, prop.data + prop.size) {
        let chunk = chunk?;
        match &chunk.id {
            b"FS  " => {
                props.rate = Some(u32::from_be_bytes(read_array(
                    file, chunk.data, "FS chunk",
                )?));
            }
            b"CHNL" => {
                let count = u16::from_be_bytes(read_array(file, chunk.data, "CHNL chunk")?);
                if !(1..=6).contains(&count) {
                    return Err(DsdError::malformed(format!("{count} channels")));
                }
                let mut ids = Vec::with_capacity(usize::from(count));
                for i in 0..u64::from(count) {
                    ids.push(read_array(file, chunk.data + 2 + 4 * i, "CHNL chunk")?);
                }
                props.channels = Some(ids);
            }
            b"CMPR" => {
                let kind: [u8; 4] = read_array(file, chunk.data, "CMPR chunk")?;
                if &kind != b"DSD " {
                    return Err(DsdError::unsupported(format!(
                        "{} compression",
                        String::from_utf8_lossy(&kind).trim_end()
                    )));
                }
            }
            _ => {}
        }
    }
    Ok(props)
}

pub(super) fn parse(file: &File, file_len: u64) -> Result<Parsed, DsdError> {
    let head: [u8; 16] = read_array(file, 0, "FRM8 header")?;
    if &head[0..4] != b"FRM8" || &head[12..16] != b"DSD " {
        return Err(DsdError::NotDsd);
    }
    let end = u64_at(&head, 4).saturating_add(12).min(file_len);
    let mut props = None;
    let mut sound = None;
    for chunk in chunks(file, 16, end) {
        let chunk = chunk?;
        match &chunk.id {
            b"PROP" => props = Some(properties(file, chunk)?),
            b"DSD " => sound = Some(chunk),
            b"DST " => return Err(DsdError::unsupported("DST compression")),
            _ => {}
        }
        if props.is_some() && sound.is_some() {
            break;
        }
    }
    let props = props.ok_or_else(|| DsdError::malformed("no PROP chunk"))?;
    let sound = sound.ok_or_else(|| DsdError::malformed("no DSD sound data chunk"))?;
    let dsd_rate = props
        .rate
        .filter(|&r| r > 0)
        .ok_or_else(|| DsdError::malformed("no sample rate"))?;
    let ids = props
        .channels
        .ok_or_else(|| DsdError::malformed("no CHNL chunk"))?;
    let (layout, order) = layout_of(&ids).ok_or_else(|| DsdError::malformed("no channels"))?;
    let data_len = sound.size.min(file_len.saturating_sub(sound.data));
    let channels = ids.len() as u64;
    Ok(Parsed {
        info: DsdInfo {
            container: DsdContainer::Dff,
            dsd_rate,
            layout,
            samples: data_len / channels * 8,
        },
        data_start: sound.data,
        data_len,
        storage: Storage::Interleaved,
        order,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_are_put_in_wave_order() {
        let (layout, order) = layout_of(&[*b"SLFT", *b"SRGT"]).unwrap();
        assert_eq!(layout, ChannelLayout::STEREO);
        assert_eq!(order, [0, 1]);
        // A 5.1 file stored as L, R, LS, RS, C, LFE.
        let (layout, order) =
            layout_of(&[*b"MLFT", *b"MRGT", *b"LS  ", *b"RS  ", *b"C   ", *b"LFE "]).unwrap();
        assert_eq!(layout, ChannelLayout::SURROUND_5_1);
        assert_eq!(order, [0, 1, 4, 5, 2, 3]);
        // Unknown identifiers keep the file's order.
        let (layout, order) = layout_of(&[*b"C001", *b"C002", *b"C003"]).unwrap();
        assert_eq!(layout.channel_count(), 3);
        assert_eq!(order, [0, 1, 2]);
    }
}
