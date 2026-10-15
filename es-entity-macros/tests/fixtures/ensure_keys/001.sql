CREATE TABLE widgets (
    id UUID PRIMARY KEY,
    code VARCHAR NOT NULL UNIQUE,
    account VARCHAR NOT NULL,
    currency VARCHAR NOT NULL,
    reference VARCHAR NOT NULL,
    lowered VARCHAR NOT NULL,
    note VARCHAR NOT NULL,
    UNIQUE (account, currency)
);
CREATE UNIQUE INDEX widgets_reference_idx ON widgets (reference) WHERE reference <> '';
CREATE UNIQUE INDEX widgets_lowered_idx ON widgets (lower(lowered));
