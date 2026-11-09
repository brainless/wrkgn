CREATE TABLE processing_runs (
 id INTEGER PRIMARY KEY, thread_id INTEGER NOT NULL, started_at INTEGER NOT NULL,
 finished_at INTEGER, status TEXT NOT NULL, prompt_version TEXT NOT NULL,
 schema_version TEXT NOT NULL, model TEXT NOT NULL, error TEXT
);
CREATE TABLE processing_groups (
 root_item_id INTEGER PRIMARY KEY, thread_id INTEGER NOT NULL,
 fingerprint TEXT NOT NULL, prompt_version TEXT NOT NULL, schema_version TEXT NOT NULL,
 model TEXT NOT NULL, status TEXT NOT NULL, run_id INTEGER NOT NULL REFERENCES processing_runs(id),
 item_count INTEGER NOT NULL, processed_item_count INTEGER NOT NULL, error TEXT
);
CREATE TABLE projects (
 id INTEGER PRIMARY KEY, identity_key TEXT NOT NULL UNIQUE, slug TEXT NOT NULL UNIQUE,
 created_at INTEGER NOT NULL, suppressed INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE mentions (
 id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id),
 root_item_id INTEGER NOT NULL, source_item_id INTEGER NOT NULL REFERENCES source_items(item_id),
 mention_key TEXT NOT NULL UNIQUE, name TEXT, description TEXT NOT NULL,
 problem TEXT, audience TEXT, builder_username TEXT, review_state TEXT NOT NULL DEFAULT 'pending',
 reviewed_at INTEGER, source_fingerprint TEXT NOT NULL
);
CREATE INDEX mentions_project ON mentions(project_id);
CREATE INDEX mentions_root ON mentions(root_item_id);
CREATE TABLE mention_links (mention_id INTEGER NOT NULL REFERENCES mentions(id) ON DELETE CASCADE,
 url TEXT NOT NULL, PRIMARY KEY(mention_id,url));
CREATE TABLE mention_labels (mention_id INTEGER NOT NULL REFERENCES mentions(id) ON DELETE CASCADE,
 kind TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY(mention_id,kind,value));
CREATE TABLE mention_requests (mention_id INTEGER NOT NULL REFERENCES mentions(id) ON DELETE CASCADE,
 kind TEXT NOT NULL, source_item_id INTEGER NOT NULL REFERENCES source_items(item_id),
 evidence TEXT NOT NULL, PRIMARY KEY(mention_id,kind,source_item_id,evidence));
CREATE TABLE supporting_contributions (mention_id INTEGER NOT NULL REFERENCES mentions(id) ON DELETE CASCADE,
 source_item_id INTEGER NOT NULL REFERENCES source_items(item_id), summary TEXT NOT NULL,
 excerpt TEXT, PRIMARY KEY(mention_id,source_item_id,summary));
CREATE TABLE review_corrections (mention_key TEXT NOT NULL, field TEXT NOT NULL, value TEXT,
 PRIMARY KEY(mention_key,field));
CREATE TABLE source_suppression (source_item_id INTEGER PRIMARY KEY REFERENCES source_items(item_id),
 reason TEXT NOT NULL);
CREATE TABLE project_matches (identity_key TEXT PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id),
 reason TEXT NOT NULL);
CREATE TABLE processing_failures (
 run_id INTEGER NOT NULL REFERENCES processing_runs(id), root_item_id INTEGER NOT NULL,
 item_count INTEGER NOT NULL, processed_item_count INTEGER NOT NULL,
 error TEXT NOT NULL, PRIMARY KEY(run_id,root_item_id)
);
