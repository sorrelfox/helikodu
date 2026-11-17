// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! FFmpeg's metadata dictionary, used for the formats the tag library lacks
//! (Matroska, ASF/WMA, DSF, DFF, TAK, TTA) or fails on.
//!
//! FFmpeg renames the common fields of each format to generic keys (`title`,
//! `album_artist`, `track`, …) and passes the rest through under their own names:
//! ID3v2 frame IDs (DSF), `TXXX` descriptions, APEv2 keys (TAK, TTA), Matroska tag
//! names and ASF attribute names. The lookup tries the generic keys, then the ASF
//! names, then the ID3v2 frame and `TXXX` tables, then the Vorbis comment names.

use super::{Table, id3v2, lookup, vorbis};
use crate::tags::field::Field;

const GENERIC: Table = &[
    ("title", Field::Title),
    ("artist", Field::Artist),
    ("album", Field::Album),
    ("album_artist", Field::AlbumArtist),
    ("composer", Field::Composer),
    ("performer", Field::Performer),
    ("date", Field::Date),
    ("genre", Field::Genre),
    ("track", Field::TrackNumber),
    ("disc", Field::DiscNumber),
    ("comment", Field::Comment),
    ("copyright", Field::Copyright),
    ("encoded_by", Field::EncodedBy),
    ("publisher", Field::Label),
    ("language", Field::Language),
    ("lyrics", Field::Lyrics),
    ("compilation", Field::Compilation),
    ("grouping", Field::Grouping),
    ("title-sort", Field::TitleSort),
    ("artist-sort", Field::ArtistSort),
    ("album-sort", Field::AlbumSort),
    ("album_artist-sort", Field::AlbumArtistSort),
    ("composer-sort", Field::ComposerSort),
    ("encoder", Field::Ignored),
    ("creation_time", Field::Ignored),
    ("major_brand", Field::Ignored),
    ("minor_version", Field::Ignored),
    ("compatible_brands", Field::Ignored),
    ("handler_name", Field::Ignored),
    ("vendor_id", Field::Ignored),
    ("duration", Field::Ignored),
    ("filename", Field::Ignored),
    ("mimetype", Field::Ignored),
    ("service_name", Field::Ignored),
    ("service_provider", Field::Ignored),
    ("iTunes_CDDB_IDs", Field::Ignored),
    // Matroska tag names that are not Vorbis names.
    ("PART_NUMBER", Field::TrackNumber),
    ("TOTAL_PARTS", Field::TotalTracks),
    ("DATE_RELEASED", Field::Date),
    ("DATE_RECORDED", Field::Date),
    ("LEAD_PERFORMER", Field::Performer),
    ("ORIGINAL_MEDIA_TYPE", Field::Media),
    ("LABEL_CODE", Field::Ignored),
    ("_STATISTICS_TAGS", Field::Ignored),
    ("_STATISTICS_WRITING_APP", Field::Ignored),
    ("_STATISTICS_WRITING_DATE_UTC", Field::Ignored),
    ("BPS", Field::Ignored),
    ("NUMBER_OF_FRAMES", Field::Ignored),
    ("NUMBER_OF_BYTES", Field::Ignored),
];

/// ASF attribute names that FFmpeg passes through, as Picard writes them.
const ASF: Table = &[
    ("WM/ARTISTS", Field::Artists),
    ("WM/ArtistSortOrder", Field::ArtistSort),
    ("WM/AlbumSortOrder", Field::AlbumSort),
    ("WM/TitleSortOrder", Field::TitleSort),
    ("WM/AlbumArtistSortOrder", Field::AlbumArtistSort),
    ("WM/ComposerSortOrder", Field::ComposerSort),
    ("WM/Conductor", Field::Conductor),
    ("WM/Writer", Field::Lyricist),
    ("WM/ModifiedBy", Field::Remixer),
    ("WM/Producer", Field::Producer),
    ("WM/ContentGroupDescription", Field::Grouping),
    ("WM/SubTitle", Field::Subtitle),
    ("WM/SetSubTitle", Field::DiscSubtitle),
    ("WM/IsCompilation", Field::Compilation),
    ("WM/Lyrics", Field::Lyrics),
    ("WM/Lyrics_Synchronised", Field::Lyrics),
    ("WM/OriginalReleaseYear", Field::OriginalYear),
    ("WM/OriginalReleaseTime", Field::OriginalDate),
    ("WM/OriginalAlbumTitle", Field::OriginalAlbum),
    ("WM/OriginalArtist", Field::OriginalArtist),
    ("WM/Barcode", Field::Barcode),
    ("WM/CatalogNo", Field::CatalogNumber),
    ("WM/Media", Field::Media),
    ("WM/Mood", Field::Mood),
    ("WM/ISRC", Field::Isrc),
    ("WM/BeatsPerMinute", Field::Bpm),
    ("WM/InitialKey", Field::Key),
    ("WM/Script", Field::Script),
    ("WM/EncodingSettings", Field::EncoderSettings),
    ("WM/SharedUserRating", Field::WmpRating),
    ("WM/Director", Field::Director),
    ("WM/AuthorURL", Field::Website),
    ("WM/TrackNumber", Field::TrackNumber),
    ("WM/PartOfSet", Field::DiscNumber),
    ("WM/Year", Field::Date),
    ("WM/Composer", Field::Composer),
    ("WM/Genre", Field::Genre),
    ("WM/AlbumArtist", Field::AlbumArtist),
    ("WM/AlbumTitle", Field::Album),
    ("WM/Publisher", Field::Label),
    ("WM/EncodedBy", Field::EncodedBy),
    ("WM/Language", Field::Language),
    ("WM/Picture", Field::Ignored),
    ("WM/Track", Field::Ignored),
    ("WM/MediaPrimaryClassID", Field::Ignored),
    ("WM/WMContentID", Field::Ignored),
    ("WM/WMCollectionID", Field::Ignored),
    ("WM/WMCollectionGroupID", Field::Ignored),
    ("WM/Provider", Field::Ignored),
    ("WM/UniqueFileIdentifier", Field::Ignored),
    ("IsVBR", Field::Ignored),
    ("DeviceConformanceTemplate", Field::Ignored),
    ("WMFSDKVersion", Field::Ignored),
    ("WMFSDKNeeded", Field::Ignored),
    ("Author", Field::Artist),
    ("Description", Field::Comment),
    ("Acoustid/Id", Field::AcoustidId),
];

/// The field of an FFmpeg metadata key.
pub(crate) fn field(key: &str) -> Option<Field> {
    if let Some(field) = lookup(GENERIC, key).or_else(|| lookup(ASF, key)) {
        return Some(field);
    }
    // ASF writes the MusicBrainz names with a slash where ID3 has a space.
    if let Some(rest) = key.strip_prefix("MusicBrainz/") {
        return id3v2::user_text_field(&format!("MusicBrainz {rest}"));
    }
    // A USLT frame becomes `lyrics-<language>`.
    if key
        .get(..7)
        .is_some_and(|p| p.eq_ignore_ascii_case("lyrics-"))
    {
        return Some(Field::Lyrics);
    }
    if key.len() == 4
        && key
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        && let Some(field) = id3v2::frame_field(key)
    {
        return Some(field);
    }
    id3v2::user_text_field(key).or_else(|| vorbis::field(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_have_no_duplicates() {
        crate::tags::mapping::assert_no_duplicates(GENERIC);
        crate::tags::mapping::assert_no_duplicates(ASF);
    }

    #[test]
    fn lookups() {
        assert_eq!(field("album_artist"), Some(Field::AlbumArtist));
        assert_eq!(field("WM/SharedUserRating"), Some(Field::WmpRating));
        assert_eq!(field("MusicBrainz/Album Id"), Some(Field::MbAlbumId));
        assert_eq!(field("lyrics-eng"), Some(Field::Lyrics));
        assert_eq!(field("TPE4"), Some(Field::Remixer));
        assert_eq!(field("TSRC"), Some(Field::Isrc));
        assert_eq!(
            field("MusicBrainz Release Track Id"),
            Some(Field::MbTrackId)
        );
        assert_eq!(field("MUSICBRAINZ_ALBUMID"), Some(Field::MbAlbumId));
        assert_eq!(
            field("REPLAYGAIN_TRACK_GAIN"),
            Some(Field::ReplayGainTrackGain)
        );
        assert_eq!(field("something else"), None);
    }
}
