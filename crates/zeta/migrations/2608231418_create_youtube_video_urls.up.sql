CREATE TABLE youtube_video_urls (
  id SERIAL,
  video_id TEXT NOT NULL,
  nickname TEXT NOT NULL,
  username TEXT NOT NULL,
  hostname TEXT NOT NULL,
  channel TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
  network_id TEXT NOT NULL
);

CREATE INDEX youtube_video_channel_network_id_idx ON youtube_video_urls (video_id, channel, network_id);
