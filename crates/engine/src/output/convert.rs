// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Conversion from the ring buffer's `f32` samples to the device's sample format
//! and channel order. The encoder lives with the rest of the processing path, in
//! [`crate::pipeline`]; the backends use it from here.

pub(crate) use crate::pipeline::Encoder;
