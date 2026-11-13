ALTER TABLE youtube_video_urls DROP CONSTRAINT youtube_video_urls_pkey;

ALTER TABLE url_records DROP CONSTRAINT url_records_pkey;

ALTER TABLE unwall_urls DROP CONSTRAINT unwall_urls_pkey;

ALTER TABLE unwall_sites DROP CONSTRAINT unwall_sites_pkey;

ALTER TABLE unwall_fetches DROP CONSTRAINT unwall_fetches_pkey;

ALTER TABLE notifications DROP CONSTRAINT notifications_pkey;

ALTER TABLE filters DROP CONSTRAINT filters_pkey;

ALTER TABLE alerts DROP CONSTRAINT alerts_pkey;
