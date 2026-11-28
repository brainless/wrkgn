-- A maintainer can acknowledge a failed coverage unit: it is skipped by resume
-- and no longer counts as a failure, but stays reported and keeps its state.
ALTER TABLE coverage_units ADD COLUMN acknowledged_at INTEGER;
ALTER TABLE coverage_units ADD COLUMN acknowledged_note TEXT
    CHECK(acknowledged_note IS NULL OR length(acknowledged_note) <= 500);
