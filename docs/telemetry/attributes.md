# Attribute registry

Every attribute zeta records, grouped by namespace. The source column says where the name comes
from:

- **OTel** — an exact OpenTelemetry semantic-convention key.
- **otel.\*** — the OpenTelemetry-reserved namespace; the two keys used here are interpreted by
  `tracing-opentelemetry`, not exported as attributes.
- **custom** — defined by zeta.

No ECS attribute is emitted; the raw IRC line is the custom, trace-level `irc.raw`.

Type is the logical type. Fields recorded with `?` are rendered with `Debug` by
`tracing-opentelemetry` and arrive as strings in OTLP even when the logical type is an array or
a struct. Fields recorded with `%` are strings on the wire regardless of the logical type.

## HTTP and network

| Attribute | Type | Meaning | Source |
| --- | --- | --- | --- |
| `http.request.method` | string | HTTP method of the request (`GET`, `HEAD`, `PUT`) | OTel |
| `http.response.status_code` | int | HTTP response status code | OTel |
| `http.response.body` | string | Truncated response body (up to 512 characters) on a failed status or JSON parse; there is no OTel string-body attribute | custom |
| `url.full` | string | Request or resource URL, redacted by `RedactedUrl`/`RedactedUrlRef` (no userinfo, query, or fragment) | OTel |
| `url.scheme` | string | Scheme of a repaired URL (`http`, `https`) | OTel |
| `url.domain` | string | Host used in a database lookup or URL record | OTel |
| `server.address` | string | Host of the HTTP endpoint or IRC server | OTel |
| `server.port` | int | Port of the HTTP endpoint (the scheme default when the URL omits it) or the IRC server (the TLS or plaintext default when unset) | OTel |
| `network.transport` | string | `tcp` on `irc.connect` | OTel |
| `network.protocol.name` | string | `irc` on `irc.connect` | OTel |

`server.*` belongs to HTTP client spans and `irc.connect`.

## Database

DB spans come from `#[instrument_database]`, which also sets the span name to
`db.query.summary`.

| Attribute | Type | Meaning | Source |
| --- | --- | --- | --- |
| `db.system.name` | string | Always `postgresql` | OTel |
| `db.namespace` | string | Database name from the connection options | OTel |
| `db.operation.name` | string | SQL verb (`SELECT`, `INSERT`, `DELETE`) | OTel |
| `db.collection.name` | string | Table the statement touches | OTel |
| `db.query.summary` | string | Span name, `{OPERATION} {collection}` | OTel |

## DNS and storage

| Attribute | Type | Meaning | Source |
| --- | --- | --- | --- |
| `dns.question.name` | string | Domain being resolved | OTel |
| `dns.answers` | array of strings | Resolved addresses | OTel |
| `aws.s3.key` | string | Object key of the S3 operation | OTel |

## Process, file, and errors

| Attribute | Type | Meaning | Source |
| --- | --- | --- | --- |
| `process.command` | string | Executable passed to `Command::new` (yt-dlp), not a command line | OTel |
| `file.path` | string | Filesystem path of a download directory, file, or upload | OTel |
| `error.type` | string | Low-cardinality failure class; see values below | OTel |
| `exception.message` | string | Rendered error message | OTel |

`error.type` values:

- Transport failures: `timeout`, `connection_error`, `other`.
- S3 failures: `s3_config`, `s3_endpoint`, `s3_io`, `s3_request`, `s3_signing`, `s3_status`.
- HTTP responses: the numeric status code as a string. The standard client does not record a 404
  (an expected miss); the emulated browser client records every non-success status.

`error.type` is omitted when the operation succeeds.

## `otel.*` instrumentation controls

| Attribute | Type | Meaning | Source |
| --- | --- | --- | --- |
| `otel.name` | string | Overrides the exported span name; used for the bounded `{plugin} {kind}` discriminator | otel.* |
| `otel.kind` | string | Sets the OTel span kind: `client` for HTTP/S3/DB and IRC connect, `producer` for outbound IRC replies, `consumer` for inbound IRC | otel.* |

These are consumed by the tracing layer and are not exported as span attributes.

## `zeta.*` application namespace

| Attribute | Type | Meaning | Source |
| --- | --- | --- | --- |
| `zeta.plugin.name` | string | Static plugin name | custom |
| `zeta.plugin.count` | int | Number of plugins in the registry | custom |
| `zeta.plugin.failed.count` | int | Plugin tasks that stopped before their `loaded` hook finished | custom |
| `zeta.plugin.failed.names` | array of strings | Names of the plugins that failed to initialize | custom |
| `zeta.plugin.compiled_out.names` | array of strings | Configured plugins whose cargo feature is not compiled in | custom |
| `zeta.plugin.section` | string | Unknown `[plugins.<name>]` configuration section | custom |
| `zeta.plugin.command_trigger` | string | Command trigger that cannot be indexed (empty or containing whitespace) | custom |
| `zeta.plugin.load.attempt` | int | Current startup retry attempt, starting at 1 | custom |
| `zeta.plugin.load.max_attempts` | int | Total startup load attempts | custom |
| `zeta.plugin.mailbox.dropped` | int | Cumulative events shed by one plugin mailbox | custom |
| `zeta.plugin.mailbox.capacity` | int | Capacity of the plugin mailbox | custom |
| `zeta.settings.key` | string | Unknown key in a plugin's settings section | custom |
| `zeta.event.kind` | enum string | Event kind on a handler span: `command`, `url`, `message`, `join`, `part`, `quit`, `nick`, `kick`, `ctcp`, `raw` | custom |
| `zeta.shutdown.reason` | string | Reason carried by the IRC server's `ERROR` command | custom |
| `zeta.mirror.public_url_base` | string | Configured public URL base that failed to parse | custom |
| `zeta.admin.hostmask.count` | int | Number of configured admin hostmasks | custom |
| `zeta.download.id` | string | Download id in one pre-start mirror diagnostic | custom |

## Messaging and IRC

| Attribute | Type | Meaning | Source |
| --- | --- | --- | --- |
| `messaging.system.name` | string | Always `irc` | OTel |
| `messaging.operation.name` | string | `privmsg` on `irc.send`; otherwise the lowercased IRC command: `privmsg`, `notice`, `join`, `part`, `quit`, `nick`, `kick`, …; an unmodeled command is lowercased when rendered | OTel |
| `messaging.operation.type` | string | `process` for inbound events, `send` on `irc.send` | OTel |
| `messaging.destination.name` | string | Channel or nick targeted by an inbound `PRIVMSG`, `NOTICE`, `JOIN`, `PART`, or `KICK`, or the reply channel on `irc.send` | OTel |
| `messaging.message.id` | string | IRCv3 `msgid` attached to any event, when present; the client requests the `message-tags` capability at registration | OTel |
| `irc.command` | string | IRC command as received, e.g. `PRIVMSG`; `ERROR` on the host's `irc.error` span | custom |
| `irc.channel` | string | Channel or nick target | custom |
| `irc.raw` | string | Full IRC line, trace level; inside `irc.dispatch` for a delivering line, stdout-only when the line has no span | custom |
| `irc.hostname` | string | Configured server hostname on `zeta.startup` | custom |
| `irc.nickname` | string | Configured bot nickname on `zeta.startup` and `irc.connect` | custom |
| `irc.sender.nick` | string | Nick attributed to a database row | custom |
| `irc.user.nick` | string | Nick of the user who posted a URL (OFN) | custom |
| `irc.user.name` | string | Username (ident) of the user who posted a URL (OFN) | custom |
| `irc.user.host` | string | Hostname of the user who posted a URL (OFN) | custom |
| `irc.network` | string | Network identifier of a URL record (OFN) | custom |

## `media.*`

| Attribute | Type | Meaning | Source |
| --- | --- | --- | --- |
| `media.id` | string | Media identifier (mirror id, Reddit video id) | custom |
| `media.link` | string | Public URL of an already-mirrored media object | custom |
| `media.download.progress_percent` | float | Download progress percentage; absent while the total size is unknown | custom |
| `media.download.progress` | string | `Debug` rendering of the downloader's progress value | custom |
| `media.download.stale_removed.count` | int | Stale download directories removed at startup | custom |
| `media.download.queued.count` | int | Downloads waiting for a slot | custom |
| `media.download.running.count` | int | Active downloads | custom |
| `media.upload.running.count` | int | Active uploads | custom |
| `media.upload.waiting.count` | int | Uploads waiting for a slot | custom |
| `media.video.codec` | string | Video codec of a file that browsers cannot play | custom |

## Plugin namespaces

Each plugin owns its plugin name as the root namespace. `search.*`, `geo.*`, and `media.*` are
shared across plugins because they describe a general concept, not one plugin.

| Attribute | Type | Meaning | Source |
| --- | --- | --- | --- |
| `alert.count` | int | Alerts in one scheduler batch | custom |
| `alert.cutoff` | timestamp string | Due-time cutoff of an alert query | custom |
| `alert.id` | int | Alert being delivered | custom |
| `alert.ids` | array of ints | Alerts deleted in one batch | custom |
| `chaturbate.username` | string | Chaturbate username parsed from a URL | custom |
| `coinmarketcap.count` | int | Coins or fiats cached | custom |
| `coinmarketcap.limit` | int | Requested listing or map limit | custom |
| `coinmarketcap.query` | string | Quote query | custom |
| `filter.count` | int | Filters loaded or added | custom |
| `filter.ids` | array of ints | Filters deleted in one batch | custom |
| `geo.location.lat` | float | Latitude passed to the weather lookup | custom |
| `geo.location.lon` | float | Longitude passed to the weather lookup | custom |
| `geoip.lookup.address` | string | Address returned by a GeoIP lookup | custom |
| `geoip.lookup.name` | string | Name (address or domain) passed to a GeoIP lookup | custom |
| `imdb.errors` | string | GraphQL errors joined into one message | custom |
| `imdb.person.id` | string | IMDb person id | custom |
| `imdb.title.id` | string | IMDb title id | custom |
| `instagram.media.id` | string | Instagram media id | custom |
| `instagram.media.shortcode` | string | Instagram media shortcode | custom |
| `instagram.resource` | string | Instagram resource label being fetched | custom |
| `instagram.username` | string | Instagram username | custom |
| `isitopen.place.id` | string | Place id passed to the opening-hours lookup | custom |
| `kagi.endpoint` | string | Kagi stream endpoint (`search` or `images`) | custom |
| `kagi.wire_tag` | string | Wire tag of an unparseable Kagi chunk | custom |
| `notification.count` | int | Notifications deleted in one batch | custom |
| `notification.ids` | array of ints | Notifications deleted in one batch | custom |
| `ofn.statistics.date` | date string | Date of the OFN statistics query | custom |
| `reddit.client.id` | string | OAuth client id in use | custom |
| `reddit.shortlink.id` | string | Reddit shortlink id | custom |
| `reddit.subreddit.name` | string | Subreddit name | custom |
| `reddit.submission.id` | string | Reddit submission id | custom |
| `search.limit` | int | Result limit of a search | custom |
| `search.query` | string | Search terms | custom |
| `spotify.resource.type` | string | Unsupported Spotify resource type | custom |
| `thingiverse.thing.id` | string | Thingiverse thing id | custom |
| `tiktok.short.id` | string | TikTok short-link id | custom |
| `unwall.site.count` | int | Unwall sites loaded or added | custom |
| `unwall.site.pattern` | string | Site pattern being removed | custom |
| `unwall.statistics.date` | date string | Date of the unwall statistics query | custom |
| `urban_dictionary.definition.count` | int | Definitions returned for a term | custom |
| `urban_dictionary.term` | string | Term queried from Urban Dictionary | custom |

## Other application keys

| Attribute | Type | Meaning | Source |
| --- | --- | --- | --- |
| `config.path` | string | Configuration file path on `zeta.startup` | custom |
| `signal` | string | Shutdown signal (`SIGINT`, `SIGTERM`) on `zeta.signal` and `zeta.signal.second` | custom |
