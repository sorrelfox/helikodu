-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- The fingerprint of a track identity, so that a track that leaves the library and comes
-- back under another path finds its statistics, rating and history again.
--
-- - `content_hash` is the quick hash of the file (the size and the first and last
--   64 KiB, as in `files.quick_hash`) when the identity was created or last confirmed.
-- - `audio_key` encodes the audio properties of the track (the part of the file, the
--   duration, the sample rate and the channel count), which tells apart the tracks of
--   one CUE sheet and guards against a hash collision.
--
-- Identities created before this migration have neither; they keep their tracks but
-- cannot be found again by fingerprint.

ALTER TABLE identities ADD COLUMN content_hash BLOB;
ALTER TABLE identities ADD COLUMN audio_key BLOB;
CREATE INDEX identities_fingerprint ON identities (content_hash, audio_key)
    WHERE content_hash IS NOT NULL;
