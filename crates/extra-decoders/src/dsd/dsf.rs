// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The DSF header (Sony DSD Stream File, version 1). Little-endian.
//!
//! A `DSD ` chunk (28 bytes), a `fmt ` chunk (52 bytes) and a `data` chunk whose
//! sound data is split into per-channel blocks, usually of 4096 bytes: one block of
//! each channel in turn. The last block of each channel is padded with zeros.

use std::fs::File;

use helikodu_model::audio::{ChannelLayout, ChannelPosition};

use super::error::DsdError;
use super::reader::{DsdContainer, DsdInfo, Parsed, Storage, read_array};

/// The largest block size accepted, per channel.
const MAX_BLOCK_SIZE: u32 = 1 << 20;

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    let mut b = [0u8; 4];
    b.copy_from_slice(&bytes[at..at + 4]);
    u32::from_le_bytes(b)
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(b)
}

/// The channel layout of a DSF channel type.
fn layout_of(channel_type: u32, channels: u16) -> Option<ChannelLayout> {
    use ChannelPosition::{FrontCenter, FrontLeft, FrontRight, LowFrequency};
    let layout = match channel_type {
        1 => Some(ChannelLayout::MONO),
        2 => Some(ChannelLayout::STEREO),
        3 => Some(ChannelLayout::SURROUND_3_0),
        4 => Some(ChannelLayout::QUAD),
        5 => ChannelLayout::from_positions([FrontLeft, FrontRight, FrontCenter, LowFrequency]),
        6 => Some(ChannelLayout::SURROUND_5_0),
        7 => Some(ChannelLayout::SURROUND_5_1),
        _ => None,
    };
    layout
        .filter(|l| l.channel_count() == channels)
        .or_else(|| ChannelLayout::default_for_count(channels))
}

pub(super) fn parse(file: &File, file_len: u64) -> Result<Parsed, DsdError> {
    let head: [u8; 28] = read_array(file, 0, "DSD chunk")?;
    if &head[0..4] != b"DSD " {
        return Err(DsdError::NotDsd);
    }
    let fmt_at = u64_at(&head, 4);
    if fmt_at < 28 || fmt_at >= file_len {
        return Err(DsdError::malformed("the DSD chunk has a wrong size"));
    }
    let fmt: [u8; 52] = read_array(file, fmt_at, "fmt chunk")?;
    if &fmt[0..4] != b"fmt " {
        return Err(DsdError::malformed("no fmt chunk after the DSD chunk"));
    }
    let fmt_size = u64_at(&fmt, 4);
    let format_id = u32_at(&fmt, 16);
    let channel_type = u32_at(&fmt, 20);
    let channel_count = u32_at(&fmt, 24);
    let dsd_rate = u32_at(&fmt, 28);
    let bits_per_sample = u32_at(&fmt, 32);
    let samples = u64_at(&fmt, 36);
    let block_size = u32_at(&fmt, 44);
    if fmt_size < 52 {
        return Err(DsdError::malformed("the fmt chunk is too short"));
    }
    if format_id != 0 {
        return Err(DsdError::unsupported(format!("DSF format {format_id}")));
    }
    let channels = u16::try_from(channel_count)
        .ok()
        .filter(|c| (1..=6).contains(c))
        .ok_or_else(|| DsdError::malformed(format!("{channel_count} channels")))?;
    let lsb_first = match bits_per_sample {
        1 => true,
        8 => false,
        other => return Err(DsdError::malformed(format!("{other} bits per sample"))),
    };
    if dsd_rate == 0 {
        return Err(DsdError::malformed("no sample rate"));
    }
    if block_size == 0 || block_size > MAX_BLOCK_SIZE {
        return Err(DsdError::malformed(format!("block size {block_size}")));
    }
    let layout = layout_of(channel_type, channels)
        .ok_or_else(|| DsdError::malformed("no channel layout"))?;

    let data_at = fmt_at
        .checked_add(fmt_size)
        .ok_or_else(|| DsdError::malformed("the fmt chunk has a wrong size"))?;
    let data: [u8; 12] = read_array(file, data_at, "data chunk")?;
    if &data[0..4] != b"data" {
        return Err(DsdError::malformed("no data chunk after the fmt chunk"));
    }
    let data_start = data_at + 12;
    let data_len = u64_at(&data, 4)
        .saturating_sub(12)
        .min(file_len.saturating_sub(data_start));

    Ok(Parsed {
        info: DsdInfo {
            container: DsdContainer::Dsf,
            dsd_rate,
            layout,
            samples,
        },
        data_start,
        data_len,
        storage: Storage::Blocks {
            block_size: u64::from(block_size),
            lsb_first,
        },
        order: (0..usize::from(channels)).collect(),
    })
}
