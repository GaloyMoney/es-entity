CREATE TABLE tasks (
  id UUID PRIMARY KEY,
  workspace_id UUID DEFAULT NULL,
  status VARCHAR NOT NULL,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX idx_tasks_workspace_id ON tasks (workspace_id);
CREATE INDEX idx_tasks_status ON tasks (status);

CREATE TABLE task_events (
  id UUID NOT NULL REFERENCES tasks(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
