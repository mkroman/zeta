CREATE INDEX alerts_channel_nickname_time_idx ON alerts (
  channel,
  nickname,
  time
);
