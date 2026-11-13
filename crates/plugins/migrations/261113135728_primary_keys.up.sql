-- The original table definitions declared `id SERIAL` without a primary key, so the surrogate
-- key was neither unique nor indexed. The repositories already treat it as an identity.
ALTER TABLE alerts ADD CONSTRAINT alerts_pkey PRIMARY KEY (id);

ALTER TABLE filters ADD CONSTRAINT filters_pkey PRIMARY KEY (id);

ALTER TABLE notifications ADD CONSTRAINT notifications_pkey PRIMARY KEY (id);

ALTER TABLE unwall_fetches ADD CONSTRAINT unwall_fetches_pkey PRIMARY KEY (id);

ALTER TABLE unwall_sites ADD CONSTRAINT unwall_sites_pkey PRIMARY KEY (id);

ALTER TABLE unwall_urls ADD CONSTRAINT unwall_urls_pkey PRIMARY KEY (id);

ALTER TABLE url_records ADD CONSTRAINT url_records_pkey PRIMARY KEY (id);

ALTER TABLE youtube_video_urls ADD CONSTRAINT youtube_video_urls_pkey PRIMARY KEY (id);
