CREATE TABLE fk_widgets (
    id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES owners(id),
    name VARCHAR NOT NULL UNIQUE
);
