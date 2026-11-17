// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Vorbis comments (FLAC, Ogg Vorbis, Opus, Speex): Picard's names, plus the common
//! spellings of other taggers. Also the fallback for APEv2, `TXXX` and MP4 freeform keys.

use super::{Table, lookup};
use crate::tags::field::Field;

const TABLE: Table = &[
    ("TITLE", Field::Title),
    ("TITLESORT", Field::TitleSort),
    ("SUBTITLE", Field::Subtitle),
    ("VERSION", Field::Subtitle),
    ("ARTIST", Field::Artist),
    ("ARTISTS", Field::Artists),
    ("ARTISTSORT", Field::ArtistSort),
    ("ALBUM", Field::Album),
    ("ALBUMSORT", Field::AlbumSort),
    ("ALBUMARTIST", Field::AlbumArtist),
    ("ALBUM ARTIST", Field::AlbumArtist),
    ("ALBUM_ARTIST", Field::AlbumArtist),
    ("ALBUMARTISTS", Field::AlbumArtists),
    ("ALBUMARTISTSORT", Field::AlbumArtistSort),
    ("TRACKNUMBER", Field::TrackNumber),
    ("TRACKTOTAL", Field::TotalTracks),
    ("TOTALTRACKS", Field::TotalTracks),
    ("DISCNUMBER", Field::DiscNumber),
    ("DISCTOTAL", Field::TotalDiscs),
    ("TOTALDISCS", Field::TotalDiscs),
    ("DISCSUBTITLE", Field::DiscSubtitle),
    ("DATE", Field::Date),
    ("YEAR", Field::Year),
    ("ORIGINALDATE", Field::OriginalDate),
    ("ORIGINALYEAR", Field::OriginalYear),
    ("RELEASEDATE", Field::ReleaseDate),
    ("GENRE", Field::Genre),
    ("COMPOSER", Field::Composer),
    ("COMPOSERSORT", Field::ComposerSort),
    ("CONDUCTOR", Field::Conductor),
    ("LYRICIST", Field::Lyricist),
    ("WRITER", Field::Writer),
    ("ARRANGER", Field::Arranger),
    ("REMIXER", Field::Remixer),
    ("MIXARTIST", Field::Remixer),
    ("PRODUCER", Field::Producer),
    ("ENGINEER", Field::Engineer),
    ("MIXER", Field::Mixer),
    ("DJMIXER", Field::DjMixer),
    ("PERFORMER", Field::Performer),
    ("LABEL", Field::Label),
    ("ORGANIZATION", Field::Label),
    ("PUBLISHER", Field::Label),
    ("CATALOGNUMBER", Field::CatalogNumber),
    ("BARCODE", Field::Barcode),
    ("UPC", Field::Barcode),
    ("ISRC", Field::Isrc),
    ("ASIN", Field::Asin),
    ("RELEASETYPE", Field::ReleaseType),
    ("MUSICBRAINZ_ALBUMTYPE", Field::ReleaseType),
    ("RELEASESTATUS", Field::ReleaseStatus),
    ("MUSICBRAINZ_ALBUMSTATUS", Field::ReleaseStatus),
    ("RELEASECOUNTRY", Field::ReleaseCountry),
    ("MEDIA", Field::Media),
    ("SCRIPT", Field::Script),
    ("LANGUAGE", Field::Language),
    // Vorbis MUSICBRAINZ_TRACKID is the recording; the track is RELEASETRACKID.
    ("MUSICBRAINZ_TRACKID", Field::MbRecordingId),
    ("MUSICBRAINZ_RELEASETRACKID", Field::MbTrackId),
    ("MUSICBRAINZ_ALBUMID", Field::MbAlbumId),
    ("MUSICBRAINZ_ARTISTID", Field::MbArtistId),
    ("MUSICBRAINZ_ALBUMARTISTID", Field::MbAlbumArtistId),
    ("MUSICBRAINZ_RELEASEGROUPID", Field::MbReleaseGroupId),
    ("MUSICBRAINZ_WORKID", Field::MbWorkId),
    ("MUSICBRAINZ_DISCID", Field::MbDiscId),
    ("ACOUSTID_ID", Field::AcoustidId),
    ("ACOUSTID_FINGERPRINT", Field::Ignored),
    ("COMMENT", Field::Comment),
    ("DESCRIPTION", Field::Comment),
    ("LYRICS", Field::Lyrics),
    ("UNSYNCEDLYRICS", Field::Lyrics),
    ("UNSYNCED LYRICS", Field::Lyrics),
    ("COMPILATION", Field::Compilation),
    ("GROUPING", Field::Grouping),
    ("CONTENTGROUP", Field::Grouping),
    ("WORK", Field::Work),
    ("MOVEMENTNAME", Field::Movement),
    ("MOVEMENT", Field::MovementNumber),
    ("MOVEMENTTOTAL", Field::MovementTotal),
    ("SHOWMOVEMENT", Field::ShowMovement),
    ("BPM", Field::Bpm),
    ("KEY", Field::Key),
    ("INITIALKEY", Field::Key),
    ("MOOD", Field::Mood),
    ("ENCODEDBY", Field::EncodedBy),
    ("ENCODED-BY", Field::EncodedBy),
    ("ENCODERSETTINGS", Field::EncoderSettings),
    ("COPYRIGHT", Field::Copyright),
    ("LICENSE", Field::License),
    ("WEBSITE", Field::Website),
    ("DIRECTOR", Field::Director),
    ("ORIGINALALBUM", Field::OriginalAlbum),
    ("ORIGINALARTIST", Field::OriginalArtist),
    ("REPLAYGAIN_TRACK_GAIN", Field::ReplayGainTrackGain),
    ("REPLAYGAIN_TRACK_PEAK", Field::ReplayGainTrackPeak),
    ("REPLAYGAIN_ALBUM_GAIN", Field::ReplayGainAlbumGain),
    ("REPLAYGAIN_ALBUM_PEAK", Field::ReplayGainAlbumPeak),
    ("R128_TRACK_GAIN", Field::R128TrackGain),
    ("R128_ALBUM_GAIN", Field::R128AlbumGain),
    ("RATING", Field::Rating),
    ("FMPS_RATING", Field::FmpsRating),
    ("ENCODER", Field::Ignored),
    ("ITUNNORM", Field::Ignored),
    ("ITUNSMPB", Field::Ignored),
    ("ITUNPGAP", Field::Ignored),
    ("WAVEFORMATEXTENSIBLE_CHANNEL_MASK", Field::Ignored),
];

/// The field of a Vorbis comment key.
pub(crate) fn field(key: &str) -> Option<Field> {
    lookup(TABLE, key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_has_no_duplicates() {
        crate::tags::mapping::assert_no_duplicates(TABLE);
    }

    #[test]
    fn picard_names() {
        assert_eq!(field("musicbrainz_trackid"), Some(Field::MbRecordingId));
        assert_eq!(field("MOVEMENT"), Some(Field::MovementNumber));
        assert_eq!(field("AlbumArtist"), Some(Field::AlbumArtist));
        assert_eq!(field("NOT A KEY"), None);
    }
}
