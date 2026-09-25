CREATE TABLE unwall_urls (
    id SERIAL,
    url TEXT NOT NULL,
    host TEXT NOT NULL,
    unwall_url TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE UNIQUE INDEX unwall_urls_url_idx ON unwall_urls (
    url
);

CREATE INDEX unwall_urls_host_idx ON unwall_urls (
    host
);

CREATE TABLE unwall_sites (
    id SERIAL,
    host TEXT NOT NULL,
    nickname TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE UNIQUE INDEX unwall_sites_host_idx ON unwall_sites (
    host
);

CREATE TABLE unwall_site_removals (
    id SERIAL,
    host TEXT NOT NULL,
    nickname TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE UNIQUE INDEX unwall_site_removals_host_idx ON unwall_site_removals (
    host
);

CREATE TABLE unwall_fetches (
    id SERIAL,
    url TEXT NOT NULL,
    host TEXT NOT NULL,
    nickname TEXT NOT NULL,
    username TEXT NOT NULL,
    hostname TEXT NOT NULL,
    channel TEXT NOT NULL,
    network_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX unwall_fetches_host_idx ON unwall_fetches (
    host
);

CREATE INDEX unwall_fetches_nickname_created_at_idx ON unwall_fetches (
    nickname,
    created_at
);

CREATE INDEX unwall_fetches_url_idx ON unwall_fetches (
    url
);
