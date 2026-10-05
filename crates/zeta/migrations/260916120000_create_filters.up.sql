CREATE TABLE filters (
    id SERIAL,
    channel TEXT,
    host TEXT,
    path TEXT,
    nickname TEXT,
    username TEXT,
    hostname TEXT,
    created_by TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX filters_channel_idx ON filters (
    channel
);
