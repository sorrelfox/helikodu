// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! ID3v2.3 and ID3v2.4 frames, and the descriptions of `TXXX` frames.
//!
//! ID3v2.3 frames are read after the tag library has upgraded them to their
//! ID3v2.4 names (`TYER` → `TDRC`, `TORY` → `TDOR`, `IPLS` → `TIPL`). The frames
//! that need more than a lookup are handled by the reader: `COMM`, `USLT`, `SYLT`,
//! `UFID`, `POPM`, `TIPL`, `TMCL`, and `TIT1` (the work when `GRP1` holds the
//! grouping, as iTunes writes it; otherwise the grouping).

use super::{Table, lookup, vorbis};
use crate::tags::field::Field;

const FRAMES: Table = &[
    ("TIT2", Field::Title),
    ("TSOT", Field::TitleSort),
    ("TIT3", Field::Subtitle),
    ("TPE1", Field::Artist),
    ("TSOP", Field::ArtistSort),
    ("TALB", Field::Album),
    ("TSOA", Field::AlbumSort),
    ("TPE2", Field::AlbumArtist),
    ("TSO2", Field::AlbumArtistSort),
    ("TRCK", Field::TrackNumber),
    ("TPOS", Field::DiscNumber),
    ("TSST", Field::DiscSubtitle),
    ("TDRC", Field::Date),
    ("TYER", Field::Year),
    ("TDOR", Field::OriginalDate),
    ("TORY", Field::OriginalYear),
    ("TDRL", Field::ReleaseDate),
    ("TCON", Field::Genre),
    ("TCOM", Field::Composer),
    ("TSOC", Field::ComposerSort),
    ("TPE3", Field::Conductor),
    ("TPE4", Field::Remixer),
    ("TEXT", Field::Lyricist),
    ("TPUB", Field::Label),
    ("TCMP", Field::Compilation),
    ("GRP1", Field::Grouping),
    ("MVNM", Field::Movement),
    ("MVIN", Field::MovementNumber),
    ("TBPM", Field::Bpm),
    ("TKEY", Field::Key),
    ("TLAN", Field::Language),
    ("TMED", Field::Media),
    ("TMOO", Field::Mood),
    ("TCOP", Field::Copyright),
    ("TENC", Field::EncodedBy),
    ("TSSE", Field::EncoderSettings),
    ("TSRC", Field::Isrc),
    ("TOAL", Field::OriginalAlbum),
    ("TOPE", Field::OriginalArtist),
    ("WCOP", Field::License),
    ("WOAR", Field::Website),
    ("TDAT", Field::Ignored),
    ("TIME", Field::Ignored),
    ("TLEN", Field::Ignored),
    ("TDEN", Field::Ignored),
    ("TDTG", Field::Ignored),
    ("TFLT", Field::Ignored),
    ("TOFN", Field::Ignored),
];

/// `TXXX` descriptions (and MP4 freeform names) that differ from the Vorbis
/// comment names.
const USER_TEXT: Table = &[
    ("MusicBrainz Artist Id", Field::MbArtistId),
    ("MusicBrainz Album Id", Field::MbAlbumId),
    ("MusicBrainz Album Artist Id", Field::MbAlbumArtistId),
    ("MusicBrainz Release Group Id", Field::MbReleaseGroupId),
    ("MusicBrainz Release Track Id", Field::MbTrackId),
    // Picard writes the recording to UFID in ID3 and to this name in MP4; other
    // taggers write it here in ID3 too.
    ("MusicBrainz Track Id", Field::MbRecordingId),
    ("MusicBrainz Work Id", Field::MbWorkId),
    ("MusicBrainz Disc Id", Field::MbDiscId),
    ("MusicBrainz Album Type", Field::ReleaseType),
    ("MusicBrainz Album Status", Field::ReleaseStatus),
    ("MusicBrainz Album Release Country", Field::ReleaseCountry),
    ("MusicBrainz TRM Id", Field::Ignored),
    ("MusicIP PUID", Field::Ignored),
    ("MusicMagic Fingerprint", Field::Ignored),
    ("Acoustid Id", Field::AcoustidId),
    ("Acoustid Fingerprint", Field::Ignored),
    ("ORIGINAL YEAR", Field::OriginalYear),
    ("originalyear", Field::OriginalYear),
    ("TOTALTRACKS", Field::TotalTracks),
    ("TOTALDISCS", Field::TotalDiscs),
];

/// The field of a frame ID that maps by itself.
pub(crate) fn frame_field(id: &str) -> Option<Field> {
    lookup(FRAMES, id)
}

/// The field of a `TXXX` description (or an MP4 freeform name).
pub(crate) fn user_text_field(description: &str) -> Option<Field> {
    lookup(USER_TEXT, description).or_else(|| vorbis::field(description))
}

/// The field of a `TIPL` (involved people) role, as Picard writes it.
pub(crate) fn involvement_field(role: &str) -> Option<Field> {
    const ROLES: Table = &[
        ("arranger", Field::Arranger),
        ("engineer", Field::Engineer),
        ("producer", Field::Producer),
        ("DJ-mix", Field::DjMixer),
        ("mix", Field::Mixer),
        ("composer", Field::Composer),
        ("lyricist", Field::Lyricist),
        ("conductor", Field::Conductor),
    ];
    lookup(ROLES, role.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_have_no_duplicates() {
        crate::tags::mapping::assert_no_duplicates(FRAMES);
        crate::tags::mapping::assert_no_duplicates(USER_TEXT);
    }

    #[test]
    fn lookups() {
        assert_eq!(frame_field("TPE2"), Some(Field::AlbumArtist));
        assert_eq!(frame_field("TXXX"), None);
        assert_eq!(
            user_text_field("MUSICBRAINZ ALBUM ID"),
            Some(Field::MbAlbumId)
        );
        assert_eq!(user_text_field("ARTISTS"), Some(Field::Artists));
        assert_eq!(
            user_text_field("replaygain_track_gain"),
            Some(Field::ReplayGainTrackGain)
        );
        assert_eq!(user_text_field("FMPS_Rating"), Some(Field::FmpsRating));
        assert_eq!(involvement_field("DJ-mix"), Some(Field::DjMixer));
    }
}
