CREATE TABLE IF NOT EXISTS project_url_aliases (
  author TEXT NOT NULL,
  canonical_url TEXT NOT NULL,
  project_id INTEGER NOT NULL REFERENCES projects(id),
  PRIMARY KEY (author, canonical_url, project_id)
);
CREATE INDEX IF NOT EXISTS project_url_aliases_project ON project_url_aliases(project_id);
