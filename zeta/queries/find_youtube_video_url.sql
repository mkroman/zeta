SELECT * FROM youtube_video_urls
WHERE video_id = $1
    AND channel = $2
    AND network_id = $3
