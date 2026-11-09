ALTER TABLE collection_runs
    ADD COLUMN interrupted INTEGER NOT NULL DEFAULT 0 CHECK (interrupted IN (0, 1));

-- Absence from a fully fetched child tree is distinct from an explicit HN
-- deleted/dead flag and from an item that could not be fetched.
ALTER TABLE source_items
    ADD COLUMN present_in_tree INTEGER NOT NULL DEFAULT 1 CHECK (present_in_tree IN (0, 1));

ALTER TABLE source_item_revisions
    ADD COLUMN present_in_tree INTEGER NOT NULL DEFAULT 1 CHECK (present_in_tree IN (0, 1));
