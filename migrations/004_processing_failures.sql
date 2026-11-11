-- Early version-three databases predate processing_failures. Later versions
-- already have it, so retain any existing failure history during the upgrade.
CREATE TABLE IF NOT EXISTS processing_failures (
 run_id INTEGER NOT NULL REFERENCES processing_runs(id), root_item_id INTEGER NOT NULL,
 item_count INTEGER NOT NULL, processed_item_count INTEGER NOT NULL,
 error TEXT NOT NULL, PRIMARY KEY(run_id,root_item_id)
);
