pub const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS index_meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS source_files (
  path TEXT PRIMARY KEY,
  unit_id TEXT,
  kind TEXT NOT NULL,
  hash TEXT,
  mtime INTEGER,
  size INTEGER,
  indexed_at TEXT,
  status TEXT NOT NULL,
  error_kind TEXT,
  error_message TEXT,
  error_field TEXT
);

CREATE TABLE IF NOT EXISTS units (
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  slug TEXT,
  status TEXT NOT NULL,
  priority INTEGER NOT NULL,
  kind TEXT NOT NULL,
  unit_type TEXT NOT NULL,
  feature INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  closed_at TEXT,
  close_reason TEXT,
  description TEXT,
  acceptance TEXT,
  notes TEXT,
  design TEXT,
  parent TEXT,
  assignee TEXT,
  claimed_by TEXT,
  claimed_at TEXT,
  is_archived INTEGER NOT NULL DEFAULT 0,
  verify TEXT,
  verify_fast TEXT,
  fail_first INTEGER NOT NULL DEFAULT 0,
  checkpoint TEXT,
  verify_hash TEXT,
  attempts INTEGER NOT NULL DEFAULT 0,
  max_attempts INTEGER NOT NULL,
  max_loops INTEGER,
  verify_timeout INTEGER,
  last_verified TEXT,
  stale_after TEXT,
  created_by TEXT,
  model TEXT,
  autonomy_disposition TEXT,
  outputs_json TEXT,
  on_fail_json TEXT,
  on_close_json TEXT,
  source_path TEXT NOT NULL,
  source_hash TEXT NOT NULL,
  indexed_at TEXT NOT NULL,
  FOREIGN KEY(source_path) REFERENCES source_files(path)
);

CREATE TABLE IF NOT EXISTS unit_labels (
  unit_id TEXT NOT NULL,
  label TEXT NOT NULL,
  position INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (unit_id, label),
  FOREIGN KEY(unit_id) REFERENCES units(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS unit_paths (
  unit_id TEXT NOT NULL,
  path TEXT NOT NULL,
  position INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (unit_id, path),
  FOREIGN KEY(unit_id) REFERENCES units(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS unit_dependencies (
  unit_id TEXT NOT NULL,
  dep_id TEXT NOT NULL,
  position INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (unit_id, dep_id),
  FOREIGN KEY(unit_id) REFERENCES units(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS unit_artifacts (
  unit_id TEXT NOT NULL,
  direction TEXT NOT NULL,
  artifact TEXT NOT NULL,
  position INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (unit_id, direction, artifact),
  FOREIGN KEY(unit_id) REFERENCES units(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS unit_decisions (
  unit_id TEXT NOT NULL,
  decision_index INTEGER NOT NULL,
  text TEXT NOT NULL,
  resolved INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (unit_id, decision_index),
  FOREIGN KEY(unit_id) REFERENCES units(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS unit_attempts (
  unit_id TEXT NOT NULL,
  attempt_index INTEGER NOT NULL,
  num INTEGER,
  outcome TEXT,
  notes TEXT,
  raw_json TEXT NOT NULL,
  PRIMARY KEY (unit_id, attempt_index),
  FOREIGN KEY(unit_id) REFERENCES units(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS unit_history (
  unit_id TEXT NOT NULL,
  history_index INTEGER NOT NULL,
  started_at TEXT,
  finished_at TEXT,
  status TEXT,
  exit_code INTEGER,
  raw_json TEXT NOT NULL,
  PRIMARY KEY (unit_id, history_index),
  FOREIGN KEY(unit_id) REFERENCES units(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS facts (
  unit_id TEXT PRIMARY KEY,
  last_verified TEXT,
  stale_after TEXT,
  score_hint REAL,
  FOREIGN KEY(unit_id) REFERENCES units(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS context_edges (
  from_unit_id TEXT NOT NULL,
  to_unit_id TEXT NOT NULL,
  edge_type TEXT NOT NULL,
  weight REAL NOT NULL DEFAULT 1.0,
  reason TEXT,
  PRIMARY KEY (from_unit_id, to_unit_id, edge_type)
);

CREATE TABLE IF NOT EXISTS index_diagnostics (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  severity TEXT NOT NULL,
  kind TEXT NOT NULL,
  source_path TEXT,
  unit_id TEXT,
  field TEXT,
  message TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_source_files_unit_id ON source_files(unit_id);
CREATE INDEX IF NOT EXISTS idx_source_files_status ON source_files(status);
CREATE INDEX IF NOT EXISTS idx_units_status ON units(status);
CREATE INDEX IF NOT EXISTS idx_units_parent ON units(parent);
CREATE INDEX IF NOT EXISTS idx_units_kind ON units(kind);
CREATE INDEX IF NOT EXISTS idx_unit_dependencies_dep_id ON unit_dependencies(dep_id);
CREATE INDEX IF NOT EXISTS idx_unit_artifacts_artifact ON unit_artifacts(artifact);
CREATE INDEX IF NOT EXISTS idx_index_diagnostics_unit_id ON index_diagnostics(unit_id);
CREATE INDEX IF NOT EXISTS idx_index_diagnostics_source_path ON index_diagnostics(source_path);
"#;
