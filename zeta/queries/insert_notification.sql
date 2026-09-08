INSERT INTO notifications (
  target, nickname, username, hostname, channel, message
) VALUES (
  $1, $2, $3, $4, $5, $6
) RETURNING *;
