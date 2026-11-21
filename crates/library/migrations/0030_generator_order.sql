-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- Generator playlists keep their place among the playlists and folders of their
-- folder.
--
-- - `generators.position` shares the numbering of `playlists.position` within the
--   same folder (`parent_id`): the sidebar lists the rows of a folder by it, a
--   playlist before a generator at the same number.
-- - Existing generators keep where they were shown: after the playlists of their
--   folder, by name.

ALTER TABLE generators ADD COLUMN position INTEGER;

UPDATE generators SET position =
    (SELECT coalesce(max(p.position), -1) + 1 FROM playlists p
     WHERE p.parent_id IS generators.parent_id)
    + (SELECT count(*) FROM generators g
       WHERE g.parent_id IS generators.parent_id
         AND (g.name < generators.name OR (g.name = generators.name AND g.id < generators.id)));
