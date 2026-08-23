INSERT INTO url_records (
    scheme,
    host,
    port,
    path,
    query,
    fragment,
    nickname,
    username,
    hostname,
    channel,
    network_id
) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) RETURNING id;
