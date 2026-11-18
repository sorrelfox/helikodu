// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Device capabilities from a sink node's `EnumFormat` parameters: the sample
//! formats, rates and channel counts the device behind the node accepts.

use ::pipewire as pw;
use helikodu_model::audio::{DeviceCapabilities, Endianness, SampleFormat, SampleRate};
use pw::spa;
use spa::pod::deserialize::PodDeserializer;
use spa::pod::{ChoiceValue, Pod, Value};
use spa::utils::{Choice, ChoiceEnum, Id};

/// The rates a range of rates is shown as.
const COMMON_RATES: [u32; 15] = [
    8_000, 11_025, 16_000, 22_050, 32_000, 44_100, 48_000, 88_200, 96_000, 176_400, 192_000,
    352_800, 384_000, 705_600, 768_000,
];
/// The most channels a range of channel counts is shown up to.
const MAX_CHANNELS: i32 = 32;

/// The capabilities one `EnumFormat` parameter describes, or `None` when it is not a
/// raw PCM audio format (IEC 958 passthrough or DSD, for example).
pub(super) fn capabilities(pod: &Pod) -> Option<DeviceCapabilities> {
    let (_, value) = PodDeserializer::deserialize_any_from(pod.as_bytes()).ok()?;
    let Value::Object(object) = value else {
        return None;
    };
    let mut caps = DeviceCapabilities::default();
    let (mut audio, mut raw) = (false, false);
    for property in object.properties {
        match property.key {
            spa::sys::SPA_FORMAT_mediaType => {
                audio = ids(&property.value).contains(&spa::sys::SPA_MEDIA_TYPE_audio);
            }
            spa::sys::SPA_FORMAT_mediaSubtype => {
                raw = ids(&property.value).contains(&spa::sys::SPA_MEDIA_SUBTYPE_raw);
            }
            spa::sys::SPA_FORMAT_AUDIO_format => {
                caps.formats = ids(&property.value)
                    .into_iter()
                    .filter_map(sample_format)
                    .collect();
            }
            spa::sys::SPA_FORMAT_AUDIO_rate => {
                caps.sample_rates = ints(&property.value, &COMMON_RATES)
                    .into_iter()
                    .filter_map(|hz| SampleRate::new(u32::try_from(hz).ok()?))
                    .collect();
            }
            spa::sys::SPA_FORMAT_AUDIO_channels => {
                let counts: Vec<u32> = (1..=MAX_CHANNELS.unsigned_abs()).collect();
                caps.channel_counts = ints(&property.value, &counts)
                    .into_iter()
                    .filter_map(|n| u16::try_from(n).ok())
                    .filter(|n| *n > 0)
                    .collect();
            }
            _ => {}
        }
    }
    (audio && raw).then(|| {
        normalize(&mut caps);
        caps
    })
}

/// Adds `more` to `caps`, keeping each list sorted and without repeats.
pub(super) fn merge(caps: &mut DeviceCapabilities, more: DeviceCapabilities) {
    caps.sample_rates.extend(more.sample_rates);
    caps.formats.extend(more.formats);
    caps.channel_counts.extend(more.channel_counts);
    normalize(caps);
}

fn normalize(caps: &mut DeviceCapabilities) {
    caps.sample_rates.sort_unstable_by_key(|rate| rate.hz());
    caps.sample_rates.dedup();
    caps.channel_counts.sort_unstable();
    caps.channel_counts.dedup();
    let mut formats = Vec::with_capacity(caps.formats.len());
    for format in caps.formats.drain(..) {
        if !formats.contains(&format) {
            formats.push(format);
        }
    }
    caps.formats = formats;
}

/// The values of an id property: a plain id, or every value of a choice.
fn ids(value: &Value) -> Vec<u32> {
    match value {
        Value::Id(Id(id)) => vec![*id],
        Value::Choice(ChoiceValue::Id(Choice(_, choice))) => match choice {
            ChoiceEnum::None(Id(id)) => vec![*id],
            ChoiceEnum::Enum {
                default,
                alternatives,
            } => std::iter::once(default)
                .chain(alternatives)
                .map(|id| id.0)
                .collect(),
            ChoiceEnum::Flags { default, flags } => std::iter::once(default)
                .chain(flags)
                .map(|id| id.0)
                .collect(),
            ChoiceEnum::Range { default, .. } | ChoiceEnum::Step { default, .. } => {
                vec![default.0]
            }
        },
        _ => Vec::new(),
    }
}

/// The values of an integer property: a plain value, the values of an enumeration,
/// or those of `common` within a range.
fn ints(value: &Value, common: &[u32]) -> Vec<i32> {
    let within = |min: i32, max: i32| -> Vec<i32> {
        common
            .iter()
            .filter_map(|v| i32::try_from(*v).ok())
            .filter(|v| (min..=max).contains(v))
            .collect()
    };
    match value {
        Value::Int(v) => vec![*v],
        Value::Choice(ChoiceValue::Int(Choice(_, choice))) => match choice {
            ChoiceEnum::None(v) => vec![*v],
            ChoiceEnum::Range { min, max, .. } | ChoiceEnum::Step { min, max, .. } => {
                within(*min, *max)
            }
            ChoiceEnum::Enum {
                default,
                alternatives,
            } => std::iter::once(default)
                .chain(alternatives)
                .copied()
                .collect(),
            ChoiceEnum::Flags { default, .. } => vec![*default],
        },
        _ => Vec::new(),
    }
}

/// The sample format of an SPA audio format id. Planar formats count as their
/// interleaved twins: the stream converts between them.
fn sample_format(id: u32) -> Option<SampleFormat> {
    use Endianness::{Big, Little};
    use spa::sys::*;
    let native = Endianness::NATIVE;
    Some(match id {
        SPA_AUDIO_FORMAT_U8 | SPA_AUDIO_FORMAT_U8P => SampleFormat::U8,
        SPA_AUDIO_FORMAT_S16_LE => SampleFormat::S16(Little),
        SPA_AUDIO_FORMAT_S16_BE => SampleFormat::S16(Big),
        SPA_AUDIO_FORMAT_S16P => SampleFormat::S16(native),
        SPA_AUDIO_FORMAT_S24_32_LE => SampleFormat::S24(Little),
        SPA_AUDIO_FORMAT_S24_32_BE => SampleFormat::S24(Big),
        SPA_AUDIO_FORMAT_S24_32P => SampleFormat::S24(native),
        SPA_AUDIO_FORMAT_S24_LE => SampleFormat::S24Packed(Little),
        SPA_AUDIO_FORMAT_S24_BE => SampleFormat::S24Packed(Big),
        SPA_AUDIO_FORMAT_S24P => SampleFormat::S24Packed(native),
        SPA_AUDIO_FORMAT_S32_LE => SampleFormat::S32(Little),
        SPA_AUDIO_FORMAT_S32_BE => SampleFormat::S32(Big),
        SPA_AUDIO_FORMAT_S32P => SampleFormat::S32(native),
        SPA_AUDIO_FORMAT_F32_LE => SampleFormat::F32(Little),
        SPA_AUDIO_FORMAT_F32_BE => SampleFormat::F32(Big),
        SPA_AUDIO_FORMAT_F32P => SampleFormat::F32(native),
        SPA_AUDIO_FORMAT_F64_LE => SampleFormat::F64(Little),
        SPA_AUDIO_FORMAT_F64_BE => SampleFormat::F64(Big),
        SPA_AUDIO_FORMAT_F64P => SampleFormat::F64(native),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use spa::pod::serialize::PodSerializer;
    use spa::pod::{Object, Property};
    use spa::utils::ChoiceFlags;

    use super::*;

    fn pod(properties: Vec<Property>) -> Vec<u8> {
        let object = Value::Object(Object {
            type_: spa::sys::SPA_TYPE_OBJECT_Format,
            id: spa::sys::SPA_PARAM_EnumFormat,
            properties,
        });
        PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &object)
            .unwrap()
            .0
            .into_inner()
    }

    fn media(subtype: u32) -> Vec<Property> {
        vec![
            Property::new(
                spa::sys::SPA_FORMAT_mediaType,
                Value::Id(Id(spa::sys::SPA_MEDIA_TYPE_audio)),
            ),
            Property::new(spa::sys::SPA_FORMAT_mediaSubtype, Value::Id(Id(subtype))),
        ]
    }

    #[test]
    fn reads_formats_rates_and_channels() {
        let mut properties = media(spa::sys::SPA_MEDIA_SUBTYPE_raw);
        properties.push(Property::new(
            spa::sys::SPA_FORMAT_AUDIO_format,
            Value::Choice(ChoiceValue::Id(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Enum {
                    default: Id(spa::sys::SPA_AUDIO_FORMAT_S32_LE),
                    alternatives: vec![
                        Id(spa::sys::SPA_AUDIO_FORMAT_S32_LE),
                        Id(spa::sys::SPA_AUDIO_FORMAT_S24_LE),
                        Id(spa::sys::SPA_AUDIO_FORMAT_S16_LE),
                        Id(spa::sys::SPA_AUDIO_FORMAT_ALAW),
                    ],
                },
            ))),
        ));
        properties.push(Property::new(
            spa::sys::SPA_FORMAT_AUDIO_rate,
            Value::Choice(ChoiceValue::Int(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Range {
                    default: 48_000,
                    min: 44_100,
                    max: 96_000,
                },
            ))),
        ));
        properties.push(Property::new(
            spa::sys::SPA_FORMAT_AUDIO_channels,
            Value::Int(2),
        ));
        let bytes = pod(properties);
        let caps = capabilities(Pod::from_bytes(&bytes).unwrap()).unwrap();
        assert_eq!(
            caps.formats,
            [
                SampleFormat::S32(Endianness::Little),
                SampleFormat::S24Packed(Endianness::Little),
                SampleFormat::S16(Endianness::Little),
            ]
        );
        let rates: Vec<u32> = caps.sample_rates.iter().map(|r| r.hz()).collect();
        assert_eq!(rates, [44_100, 48_000, 88_200, 96_000]);
        assert_eq!(caps.channel_counts, [2]);

        let mut merged = caps.clone();
        merge(
            &mut merged,
            DeviceCapabilities {
                sample_rates: vec![SampleRate::HZ_44100, SampleRate::new(192_000).unwrap()],
                formats: vec![SampleFormat::F32(Endianness::Little)],
                channel_counts: vec![6, 2],
            },
        );
        assert_eq!(merged.sample_rates.len(), 5);
        assert_eq!(merged.formats.len(), 4);
        assert_eq!(merged.channel_counts, [2, 6]);
    }

    #[test]
    fn channel_ranges_and_other_media_types() {
        let mut properties = media(spa::sys::SPA_MEDIA_SUBTYPE_raw);
        properties.push(Property::new(
            spa::sys::SPA_FORMAT_AUDIO_channels,
            Value::Choice(ChoiceValue::Int(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Range {
                    default: 2,
                    min: 1,
                    max: 4,
                },
            ))),
        ));
        let bytes = pod(properties);
        let caps = capabilities(Pod::from_bytes(&bytes).unwrap()).unwrap();
        assert_eq!(caps.channel_counts, [1, 2, 3, 4]);
        assert!(caps.formats.is_empty());

        let bytes = pod(media(spa::sys::SPA_MEDIA_SUBTYPE_iec958));
        assert!(capabilities(Pod::from_bytes(&bytes).unwrap()).is_none());
    }
}
