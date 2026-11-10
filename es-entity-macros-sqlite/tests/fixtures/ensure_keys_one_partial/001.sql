CREATE TABLE widgets (
    id UUID PRIMARY KEY,
    code VARCHAR NOT NULL UNIQUE,
    handle VARCHAR NOT NULL,
    retired BOOL NOT NULL DEFAULT FALSE
);
CREATE UNIQUE INDEX widgets_live_handle_idx ON widgets (handle) WHERE NOT retired;
