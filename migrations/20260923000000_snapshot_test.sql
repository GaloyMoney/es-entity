-- flat + nested child
CREATE TABLE meters (id UUID PRIMARY KEY, site_id UUID, label VARCHAR NOT NULL, created_at TIMESTAMPTZ NOT NULL);
CREATE INDEX idx_meters_site_id ON meters (site_id);
CREATE TABLE meter_events (id UUID NOT NULL REFERENCES meters(id), sequence INT NOT NULL, event_type VARCHAR NOT NULL, event JSONB NOT NULL, context JSONB DEFAULT NULL, recorded_at TIMESTAMPTZ NOT NULL, UNIQUE(id, sequence));
CREATE TABLE meter_snapshots (id UUID PRIMARY KEY REFERENCES meters(id), sequence INT NOT NULL, fingerprint BIGINT NOT NULL, snapshot JSONB NOT NULL, first_recorded_at TIMESTAMPTZ NOT NULL, recorded_at TIMESTAMPTZ NOT NULL);

-- nested parent
CREATE TABLE sites (id UUID PRIMARY KEY, created_at TIMESTAMPTZ NOT NULL);
CREATE TABLE site_events (id UUID NOT NULL REFERENCES sites(id), sequence INT NOT NULL, event_type VARCHAR NOT NULL, event JSONB NOT NULL, context JSONB DEFAULT NULL, recorded_at TIMESTAMPTZ NOT NULL, UNIQUE(id, sequence));
CREATE TABLE site_snapshots (id UUID PRIMARY KEY REFERENCES sites(id), sequence INT NOT NULL, fingerprint BIGINT NOT NULL, snapshot JSONB NOT NULL, first_recorded_at TIMESTAMPTZ NOT NULL, recorded_at TIMESTAMPTZ NOT NULL);

-- forgettable
CREATE TABLE clients (id UUID PRIMARY KEY, created_at TIMESTAMPTZ NOT NULL);
CREATE TABLE client_events (id UUID NOT NULL REFERENCES clients(id), sequence INT NOT NULL, event_type VARCHAR NOT NULL, event JSONB NOT NULL, context JSONB DEFAULT NULL, recorded_at TIMESTAMPTZ NOT NULL, UNIQUE(id, sequence));
CREATE TABLE client_snapshots (id UUID PRIMARY KEY REFERENCES clients(id), sequence INT NOT NULL, fingerprint BIGINT NOT NULL, snapshot JSONB NOT NULL, first_recorded_at TIMESTAMPTZ NOT NULL, recorded_at TIMESTAMPTZ NOT NULL);
CREATE TABLE clients_forgettable_payloads (entity_id UUID NOT NULL REFERENCES clients(id), sequence INT NOT NULL, payload JSONB NOT NULL, UNIQUE(entity_id, sequence));
