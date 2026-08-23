SELECT * FROM url_records
WHERE
    scheme = $1
    AND host = $2
    AND port IS NOT DISTINCT FROM $3
    AND path IS NOT DISTINCT FROM $4
    AND query IS NOT DISTINCT FROM $5
    AND channel = $6
    AND network_id = $7
