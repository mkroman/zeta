INSERT INTO youtube_video_urls (
    video_id,
    nickname,
    username,
    hostname,
    channel,
    network_id
) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id;
