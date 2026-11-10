CREATE TABLE tasks (
  id BLOB PRIMARY KEY NOT NULL,
  workspace_id BLOB DEFAULT NULL,
  status TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX idx_tasks_workspace_id ON tasks (workspace_id);
CREATE INDEX idx_tasks_status ON tasks (status);

CREATE TABLE task_events (
  id BLOB NOT NULL REFERENCES tasks(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);
