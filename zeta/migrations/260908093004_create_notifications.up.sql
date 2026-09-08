CREATE TABLE notifications (
  id SERIAL,
  target TEXT NOT NULL,
  nickname TEXT NOT NULL,
  username TEXT NOT NULL,
  hostname TEXT NOT NULL,
  channel TEXT NOT NULL,
  message TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX notifications_channel_target_idx ON notifications (
  channel, target
);
