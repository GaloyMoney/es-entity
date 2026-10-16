CREATE TABLE widgets (
    id UUID PRIMARY KEY,
    code VARCHAR NOT NULL UNIQUE,
    handle VARCHAR NOT NULL,
    deleted BOOL NOT NULL DEFAULT FALSE
);
CREATE UNIQUE INDEX widgets_live_handle_idx ON widgets (handle) WHERE deleted = FALSE;
