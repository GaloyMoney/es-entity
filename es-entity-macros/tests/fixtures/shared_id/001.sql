CREATE TABLE shared_ids (id UUID PRIMARY KEY REFERENCES parents(id), UNIQUE (id));
