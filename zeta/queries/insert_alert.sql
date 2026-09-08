INSERT INTO alerts (
  nickname, username, hostname, channel, message, time
) VALUES (
  $1, $2, $3, $4, $5, $6
) RETURNING *;
