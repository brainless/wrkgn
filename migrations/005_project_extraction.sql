-- Preserve previous extraction records and review history. New fields are populated on reprocessing.
ALTER TABLE mentions ADD COLUMN source_excerpt TEXT;
CREATE TABLE mention_builders (
    mention_id INTEGER NOT NULL REFERENCES mentions(id) ON DELETE CASCADE,
    username TEXT NOT NULL,
    PRIMARY KEY (mention_id, username)
);
