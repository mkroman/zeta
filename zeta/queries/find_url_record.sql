SELECT * FROM url_records
WHERE
    scheme = $1
    AND host = $2
    AND path IS NOT DISTINCT FROM $3
    AND query IS NOT DISTINCT FROM $4
    AND channel = $5
    AND network_id = $6
