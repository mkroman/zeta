CREATE TABLE url_records (
    id SERIAL,
    scheme TEXT NOT NULL,
    host TEXT NOT NULL,
    port INTEGER,
    path TEXT,
    query TEXT,
    fragment TEXT,
    nickname TEXT NOT NULL,
    username TEXT NOT NULL,
    hostname TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    channel TEXT NOT NULL,
    network_id TEXT
);

CREATE INDEX url_records_host_path_query_channel_network_id_index ON url_records (
    host, path, query, channel, network_id
);
