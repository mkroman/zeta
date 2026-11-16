-- The per-user settings store: one row per identity, plugin, and setting.
--
-- The identity is the IRCv3 services account the user is logged into, lowercased: users who
-- have not identified have no identity, and no settings. Values are opaque strings validated
-- against the descriptors each plugin declares.
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
-- account. One row per (identity, hostmask); `last_seen` moves when the tracker observes the
-- account from a new host, `first_seen` never does.
CREATE TABLE user_hostmasks (
    identity TEXT NOT NULL,
    hostmask TEXT NOT NULL,
    first_seen TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (identity, hostmask)
);
