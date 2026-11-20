-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- The labels and the user's fields an import of user data added, so that undoing
-- the import removes exactly those.
--
-- `import_run_labels` has one row per label given by the run: `target` 0 is a
-- track, by `identity_id`; 1 an album, by its key in `item_key`; 2 an artist, by
-- the folded name in `item_key`. `import_run_fields` has one row per field the
-- run defined, with its definition as the run stored it (`kind` as in
-- `user_fields`), so that undo leaves a field the user changed since.

CREATE TABLE import_run_labels (
    run_id      INTEGER NOT NULL REFERENCES import_runs ON DELETE CASCADE,
    label_id    INTEGER NOT NULL REFERENCES labels ON DELETE CASCADE,
    target      INTEGER NOT NULL CHECK (target IN (0, 1, 2)),
    identity_id INTEGER REFERENCES identities ON DELETE CASCADE,
    item_key    TEXT
) STRICT;
CREATE INDEX import_run_labels_run ON import_run_labels (run_id);
CREATE INDEX import_run_labels_label ON import_run_labels (label_id);
CREATE INDEX import_run_labels_identity ON import_run_labels (identity_id);

CREATE TABLE import_run_fields (
    run_id     INTEGER NOT NULL REFERENCES import_runs ON DELETE CASCADE,
    name       TEXT NOT NULL,
    kind       INTEGER NOT NULL,
    definition TEXT NOT NULL,
    PRIMARY KEY (run_id, name)
) STRICT;
