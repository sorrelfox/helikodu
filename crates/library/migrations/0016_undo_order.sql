-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- One undo history for tag edits and file runs: Ctrl+Z undoes the newest of
-- either. `seq` orders the steps of `tag_journal` and the runs of `file_runs`
-- together: when the step was made, in milliseconds since the Unix epoch, kept
-- strictly increasing across both tables so that two steps never tie.
-- `file_runs.undone_seq` is the same for when a run was last undone: a tag edit
-- made after that ends what the run can do again.

ALTER TABLE tag_journal ADD COLUMN seq INTEGER NOT NULL DEFAULT 0;
ALTER TABLE file_runs ADD COLUMN seq INTEGER NOT NULL DEFAULT 0;
ALTER TABLE file_runs ADD COLUMN undone_seq INTEGER NOT NULL DEFAULT 0;

UPDATE tag_journal SET seq = at * 1000 + id % 1000;
UPDATE file_runs SET seq = started_at * 1000 + id % 1000;
UPDATE file_runs SET undone_seq = seq WHERE state = 2;
