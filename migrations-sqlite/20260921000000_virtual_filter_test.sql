CREATE TABLE vf_accounts (
  id BLOB PRIMARY KEY NOT NULL,
  status TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL
);
CREATE INDEX idx_vf_accounts_status ON vf_accounts (status);

CREATE TABLE vf_account_events (
  id BLOB NOT NULL REFERENCES vf_accounts(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

-- Side table the `flagged` virtual filter column's predicate correlates
-- against: `EXISTS (SELECT 1 FROM vf_account_flags f WHERE f.account_id =
-- vf_accounts.id)`. Not owned by the vf_accounts repo at all — it stands in
-- for another entity's table (e.g. core_obligations in the motivating
-- lana-bank case), which is exactly the point of a virtual filter column.
CREATE TABLE vf_account_flags (
  account_id BLOB NOT NULL REFERENCES vf_accounts(id),
  flag TEXT NOT NULL
);
CREATE INDEX idx_vf_account_flags_account_id ON vf_account_flags (account_id);

-- Side table for the scoped virtual-filter test, correlated the same way
-- against the existing `contacts` table (see tests/scoped_repo.rs).
CREATE TABLE contact_flags (
  contact_id BLOB NOT NULL REFERENCES contacts(id),
  flag TEXT NOT NULL
);
CREATE INDEX idx_contact_flags_contact_id ON contact_flags (contact_id);
