CREATE TABLE unwall_site_removals (
    id SERIAL,
    host TEXT NOT NULL,
    nickname TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE UNIQUE INDEX unwall_site_removals_host_idx ON unwall_site_removals (
    host
);