-- flat + nested child
CREATE TABLE meters (id BLOB PRIMARY KEY NOT NULL, site_id BLOB, label TEXT NOT NULL, created_at TEXT NOT NULL);
CREATE INDEX idx_meters_site_id ON meters (site_id);
CREATE TABLE meter_events (id BLOB NOT NULL REFERENCES meters(id), sequence INTEGER NOT NULL, event_type TEXT NOT NULL, event TEXT NOT NULL, context TEXT DEFAULT NULL, recorded_at TEXT NOT NULL, UNIQUE(id, sequence));
CREATE TABLE meter_snapshots (id BLOB PRIMARY KEY NOT NULL REFERENCES meters(id), sequence INTEGER NOT NULL, fingerprint INTEGER NOT NULL, snapshot TEXT NOT NULL, first_recorded_at TEXT NOT NULL, recorded_at TEXT NOT NULL);

-- nested parent
CREATE TABLE sites (id BLOB PRIMARY KEY NOT NULL, created_at TEXT NOT NULL);
CREATE TABLE site_events (id BLOB NOT NULL REFERENCES sites(id), sequence INTEGER NOT NULL, event_type TEXT NOT NULL, event TEXT NOT NULL, context TEXT DEFAULT NULL, recorded_at TEXT NOT NULL, UNIQUE(id, sequence));
CREATE TABLE site_snapshots (id BLOB PRIMARY KEY NOT NULL REFERENCES sites(id), sequence INTEGER NOT NULL, fingerprint INTEGER NOT NULL, snapshot TEXT NOT NULL, first_recorded_at TEXT NOT NULL, recorded_at TEXT NOT NULL);

-- forgettable
CREATE TABLE clients (id BLOB PRIMARY KEY NOT NULL, created_at TEXT NOT NULL);
CREATE TABLE client_events (id BLOB NOT NULL REFERENCES clients(id), sequence INTEGER NOT NULL, event_type TEXT NOT NULL, event TEXT NOT NULL, context TEXT DEFAULT NULL, recorded_at TEXT NOT NULL, UNIQUE(id, sequence));
CREATE TABLE client_snapshots (id BLOB PRIMARY KEY NOT NULL REFERENCES clients(id), sequence INTEGER NOT NULL, fingerprint INTEGER NOT NULL, snapshot TEXT NOT NULL, first_recorded_at TEXT NOT NULL, recorded_at TEXT NOT NULL);
CREATE TABLE clients_forgettable_payloads (entity_id BLOB NOT NULL REFERENCES clients(id), sequence INTEGER NOT NULL, payload TEXT NOT NULL, UNIQUE(entity_id, sequence));
