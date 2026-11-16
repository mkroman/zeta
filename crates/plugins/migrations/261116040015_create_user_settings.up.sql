-- The per-user settings store: one row per identity, plugin, and setting.
--
-- The identity is the stable string the settings follow: the IRCv3 `account` tag (the services
-- account the user is logged into) when the network reports one, else the lowercased
-- `user@host`. Values are opaque strings validated against the descriptors each plugin declares.
CREATE TABLE user_settings (
    identity TEXT NOT NULL,
    plugin TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (identity, plugin, key)
);

-- The hostmasks each identity has been seen from, so operators can audit which hosts used an
-- account-backed identity. One row per (identity, hostmask); `last_seen` moves on every
-- observation, `first_seen` never does.
CREATE TABLE user_hostmasks (
    identity TEXT NOT NULL,
    hostmask TEXT NOT NULL,
    first_seen TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (identity, hostmask)
);
