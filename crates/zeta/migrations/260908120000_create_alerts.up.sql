CREATE TABLE alerts (
  id SERIAL,
  nickname TEXT NOT NULL,
  username TEXT NOT NULL,
  hostname TEXT NOT NULL,
  channel TEXT NOT NULL,
  message TEXT NOT NULL,
  time TIMESTAMPTZ NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX alerts_time_idx ON alerts (
  time
);
