CREATE TABLE driver_runs (
 id INTEGER PRIMARY KEY,
 command TEXT NOT NULL CHECK(command IN ('monthly','show_hn')),
 pid INTEGER NOT NULL,
 started_at INTEGER NOT NULL,
 finished_at INTEGER,
 status TEXT NOT NULL CHECK(status IN ('running','completed','limit_reached','interrupted','stopped_on_failure','killed_detected')),
 units_done INTEGER NOT NULL DEFAULT 0,
 items_collected INTEGER NOT NULL DEFAULT 0,
 items_processed INTEGER NOT NULL DEFAULT 0,
 items_approved INTEGER NOT NULL DEFAULT 0,
 failures INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX driver_runs_command ON driver_runs(command, id);
CREATE TABLE coverage_units (
 command TEXT NOT NULL CHECK(command IN ('monthly','show_hn')),
 unit_key TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('pending','in_progress','done','failed')),
 attempts INTEGER NOT NULL DEFAULT 0,
 first_seen_at INTEGER NOT NULL,
 updated_at INTEGER NOT NULL,
 completed_at INTEGER,
 last_run_id INTEGER REFERENCES driver_runs(id),
 last_error TEXT CHECK(last_error IS NULL OR length(last_error) <= 2000),
 PRIMARY KEY(command, unit_key)
);
CREATE INDEX coverage_units_state ON coverage_units(command, state, unit_key);
CREATE TABLE run_queue (
 command TEXT NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN ('thread','story')),
 item_id INTEGER NOT NULL,
 unit_key TEXT NOT NULL,
 position INTEGER NOT NULL DEFAULT 0,
 state TEXT NOT NULL CHECK(state IN ('pending','done','failed')),
 attempts INTEGER NOT NULL DEFAULT 0,
 last_error TEXT CHECK(last_error IS NULL OR length(last_error) <= 2000),
 updated_at INTEGER NOT NULL,
 last_run_id INTEGER REFERENCES driver_runs(id),
 PRIMARY KEY(command, kind, item_id),
 FOREIGN KEY(command, unit_key) REFERENCES coverage_units(command, unit_key)
);
CREATE INDEX run_queue_unit ON run_queue(command, unit_key, state, position);
CREATE TABLE run_errors (
 id INTEGER PRIMARY KEY,
 run_id INTEGER NOT NULL REFERENCES driver_runs(id) ON DELETE CASCADE,
 signature TEXT NOT NULL,
 stage TEXT NOT NULL,
 kind TEXT NOT NULL,
 count INTEGER NOT NULL CHECK(count > 0),
 first_item_id INTEGER,
 last_item_id INTEGER,
 sample TEXT NOT NULL CHECK(length(sample) <= 2100),
 UNIQUE(run_id, signature, stage, kind)
);
