-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- What an import run did, in figures, so that a window can show an earlier
-- import as it showed the plan, and when a run was undone.
--
-- `summary` is a JSON object with the files read, how the entries matched, the
-- totals of the plan and the playlists it made (NULL for runs made before this
-- migration). `undone_at` is the time the run was undone, NULL while it is not.

ALTER TABLE import_runs ADD COLUMN summary TEXT;
ALTER TABLE import_runs ADD COLUMN undone_at INTEGER;
