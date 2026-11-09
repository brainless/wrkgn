CREATE TABLE collection_runs (
    id INTEGER PRIMARY KEY,
    thread_id INTEGER NOT NULL CHECK (thread_id > 0),
    started_at INTEGER NOT NULL,
    finished_at INTEGER,
    status TEXT NOT NULL CHECK (status IN ('running', 'complete', 'incomplete')),
    items_fetched INTEGER NOT NULL DEFAULT 0,
    failure_count INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX collection_runs_thread_started ON collection_runs(thread_id, started_at DESC);

CREATE TABLE source_items (
    item_id INTEGER PRIMARY KEY CHECK (item_id > 0),
    thread_id INTEGER NOT NULL CHECK (thread_id > 0),
    parent_id INTEGER,
    sibling_position INTEGER NOT NULL CHECK (sibling_position >= 0),
    author TEXT,
    created_at INTEGER,
    body TEXT,
    raw_payload TEXT NOT NULL,
    content_fingerprint TEXT NOT NULL,
    deleted INTEGER NOT NULL CHECK (deleted IN (0, 1)),
    dead INTEGER NOT NULL CHECK (dead IN (0, 1)),
    first_collected_at INTEGER NOT NULL,
    last_fetched_at INTEGER NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    last_run_id INTEGER NOT NULL REFERENCES collection_runs(id)
);

CREATE INDEX source_items_thread ON source_items(thread_id);
CREATE INDEX source_items_parent_order ON source_items(parent_id, sibling_position, item_id);

-- Each revision is a complete source snapshot. Unchanged refreshes update
-- source_items.last_fetched_at without creating a duplicate revision.
CREATE TABLE source_item_revisions (
    item_id INTEGER NOT NULL REFERENCES source_items(item_id),
    revision INTEGER NOT NULL CHECK (revision > 0),
    run_id INTEGER NOT NULL REFERENCES collection_runs(id),
    changed_at INTEGER NOT NULL,
    thread_id INTEGER NOT NULL,
    parent_id INTEGER,
    sibling_position INTEGER NOT NULL,
    author TEXT,
    created_at INTEGER,
    body TEXT,
    raw_payload TEXT NOT NULL,
    content_fingerprint TEXT NOT NULL,
    deleted INTEGER NOT NULL CHECK (deleted IN (0, 1)),
    dead INTEGER NOT NULL CHECK (dead IN (0, 1)),
    PRIMARY KEY (item_id, revision)
);

CREATE INDEX source_item_revisions_run ON source_item_revisions(run_id);

-- A missing, null, or failed API response is recorded here and never changes
-- source_items.deleted/dead. Later attempts can be recorded independently.
CREATE TABLE fetch_failures (
    id INTEGER PRIMARY KEY,
    run_id INTEGER NOT NULL REFERENCES collection_runs(id),
    item_id INTEGER NOT NULL CHECK (item_id > 0),
    kind TEXT NOT NULL CHECK (kind IN ('unavailable', 'transport', 'http', 'invalid_payload')),
    detail TEXT NOT NULL,
    occurred_at INTEGER NOT NULL
);

CREATE INDEX fetch_failures_run_item ON fetch_failures(run_id, item_id);
