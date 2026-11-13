CREATE TABLE thread_extraction_modes (
 thread_id INTEGER PRIMARY KEY REFERENCES source_items(item_id),
 mode TEXT NOT NULL CHECK(mode IN ('monthly','show_hn'))
);
INSERT INTO thread_extraction_modes(thread_id,mode)
 SELECT DISTINCT thread_id,'monthly' FROM processing_runs
 WHERE thread_id IN (SELECT item_id FROM source_items);
CREATE TABLE mention_evidence (
 mention_id INTEGER NOT NULL REFERENCES mentions(id) ON DELETE CASCADE,
 source_item_id INTEGER NOT NULL REFERENCES source_items(item_id),
 source_field TEXT NOT NULL CHECK(source_field IN ('title','url','body')),
 value TEXT NOT NULL,
 supports TEXT NOT NULL,
 source_fingerprint TEXT NOT NULL,
 authorization_source_item_id INTEGER REFERENCES source_items(item_id),
 authorization_fingerprint TEXT
);
CREATE INDEX mention_evidence_mention ON mention_evidence(mention_id);
CREATE TABLE show_chunk_cache (
 thread_id INTEGER NOT NULL, cache_key TEXT NOT NULL, output_json TEXT NOT NULL,
 PRIMARY KEY(thread_id,cache_key)
);
CREATE TABLE show_processing_results (
 thread_id INTEGER PRIMARY KEY, no_project_reason TEXT
);
