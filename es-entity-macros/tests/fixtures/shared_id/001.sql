CREATE TABLE shared_ids (
    id UUID REFERENCES parents(id) CHECK (id IS NOT NULL) UNIQUE,
    CONSTRAINT shared_ids_actual_pk PRIMARY KEY (id)
);
