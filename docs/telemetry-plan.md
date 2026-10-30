# Telemetry plan — span hierarchy and attributes

Status: implemented. Date: 2026-10-30.

Covers the span/tracing work from the performance review, the span research, and
the attribute-name review. The performance fixes (inline network I/O, mirror
uploads, filter lookups) stay in their own plan; this document only covers the
traces those fixes produce.

## Status (2026-10-30, branch `telemetry/span-hierarchy`)

Done:

- [x] P0.1 startup scope / no `run` span
- [x] P0.2 HTTP client span (one `http.client` span per call covers request, body and
  parse; `reqwest` and `wreq`, exported name is the method)
- [x] P1.1 handler span carried in the mailbox envelope, `.or_current()`
- [x] P1.2 task-lifetime spans removed (alert, rink, youtube, mirror, IRC out)
- [x] P2.1 `follows_from` links for titles/unwall and the mirror download
- [x] P2.2 DNS spans (`plugins-dig`, `plugins-geoip`) and the `wreq` client span
- [x] P2.3 shutdown span (`zeta.shutdown`, with each `zeta.plugin.shutdown`
  linked to it)
- [x] P3.2 default filter `zeta=info`
- [x] P3.3 `deployment.environment.name`
- [x] P3.5 first level pass (HTTP status split, fatal/panic → error, handled
  failures → warn/debug, client logging removed)
- [x] P3.4 for runtime, mirror, imdb, tvmaze, filter, notification, unwall,
  ofn, alert, titles, reddit and the HTTP clients
- [x] P3.1 attribute renames (`plugin`→`zeta.plugin.name`, `id` family,
  `media.id`, `count`, `query`, `body`, `channel`, `path`, `key`, `error`)
- [x] P4.1 hierarchy parentage test (`handler_span_is_a_child_of_the_dispatch_span`)
- [x] P4.2 trace-shape checks in a live backend (see §8)
- [x] P4.3 AGENTS.md conventions

Remaining: none. H8 (dispatch spans for connection-protocol lines) was fixed
during the live verification: `irc.dispatch` is created only for the commands
`EventIndex` routes, plus `ERROR` for the disconnect reason.

Goals:

1. A trace answers "what did this IRC message trigger, how long did each step
   take, and what failed?"
2. No span outlives the unit of work it measures.
3. Every external dependency (HTTP, S3, DNS, DB) has a client span.
4. Names follow the OpenTelemetry naming and semantic conventions.
5. The default configuration only creates spans where they are useful.

Non-goals: metrics, log backends, a Collector deployment, changing the plugin
event model beyond the mailbox envelope needed for context propagation.

---

## 1. Current hierarchy (as built)

`→` is parent/child, `~` is lifetime, `[gap]` is an operation with no span.

```
startup                         main.rs:32      ~process lifetime
└── run                         zeta.rs:268     ~process lifetime
    ├── irc_outgoing            zeta.rs:95      ~process lifetime
    ├── dispatch                zeta.rs:119     ~per IRC line
    │   └── [no children: handler spans are roots]
    ├── second_signal           zeta.rs:159     ~until second signal
    │
    ├── loaded                  task.rs:54      root, ~per plugin startup
    │   ├── alert_scheduler     alert/service.rs:143  ~process lifetime
    │   ├── alert_delivery      alert/lib.rs:154      ~process lifetime
    │   ├── currency_refresh    rink/lib.rs:256       ~process lifetime
    │   ├── rink_actor          rink/actor.rs:61      ~process lifetime
    │   └── video_category_refresh youtube/lib.rs:397 ~process lifetime
    │
    ├── handle_event            task.rs:70      ROOT, ~per event
    │   ├── ofn.process_url     ofn/lib.rs:49
    │   │   ├── SELECT url_records     instrument_database
    │   │   └── INSERT url_records     instrument_database
    │   ├── imdb.title/person/search   imdb/client.rs:142,159,178
    │   ├── parse_response      http/response.rs:86   ← starts AFTER the request
    │   │   └── [gap] zeta_http::send / wreq send
    │   ├── fetch_page_metadata titles/lib.rs:164     child outlives parent
    │   └── unwall_article      unwall/lib.rs:415     child outlives parent
    │       └── resolve         unwall/service.rs:64
    │
    ├── shutdown                task.rs:81      ROOT, ~per plugin
    │
    └── (first mirror request)
        download_manager        manager.rs:138  child of a handler, ~process lifetime
        └── download_media      manager.rs:273  child of manager, ~per download
            └── [gap] S3 object_exists / upload_file
```

Observed on startup: `startup` opens before config/DB work and closes when the
bot stops (it wraps `run`, `main.rs:63`), and `run` wraps the whole session.
Every `dispatch` span is a child of a weeks-long root. Every `handle_event` is a
separate one-span trace. Background task spans are children of `loaded`, which
ends immediately.

---

## 2. Findings

### H1 (high) — process-lifetime root spans

`startup` covers `run` because the async block passed to
`startup.instrument(startup_span)` includes `Zeta::run` (`main.rs:39-63`), and
`Zeta::run` is itself instrumented (`zeta.rs:268`). The result is a single trace
whose root lives for the process lifetime.

Cost: the root (and `run`) is only exported when the bot stops; a crash loses
it and leaves every child orphaned; tail/parent-based sampling cannot work; the
trace view for every message is nested under a weeks-long bar; backends that
cap span/trace duration will drop or truncate it.

Fix: `startup` ends after `Zeta::new` (config, DB connect/migrate, plugin
construction). Delete the `run` span; process lifetime is not a unit of work.

### H2 (high) — handler spans are orphans; `dispatch` has no children

`handle_event` is created inside the plugin task (`task.rs:70`). Plugin tasks
are spawned from `PluginRuntime::start` (`runtime.rs:50-57`) without an
instrumented future, so `tracing`'s task-local context is empty and every
`handle_event` is a root span with its own trace ID. `dispatch` ends at enqueue,
and no event carries the dispatch context.

Cost: 5 traces per URL-less PRIVMSG instead of one; no "message → which plugins
handled it" view; handler logs (exported as span events) are attached to an
orphan trace.

Fix: carry the handler span in the mailbox envelope (`Envelope { event, span }`),
created by the dispatcher while the `dispatch` span is current and parented to it;
the plugin task runs the handler with `Instrument`. Add `.or_current()` so a
disabled handler span falls back to the dispatch span. (`#[instrument]` on the
dispatch functions cannot parent the handler: the handler runs in another task,
where no dispatch span is current.) See Phase 1.

### H3 (high) — the HTTP client span does not cover the request

`get_json`/`get_text` call `send(request).await?` and then
`parse_response(response).await` (`response.rs:208-214, 243-247`). `send`
(`client.rs:27-37`) and the emulated `send` (`emulated.rs:15-23`) have no span.
`parse_response` is instrumented (`response.rs:86`) and starts only once the
response headers exist.

Cost: the trace hides DNS/TCP/TLS/TTFB — usually the dominant part of an API
call; the `parse_response` duration is body read + parse. A request that fails
before a response (timeout, refused, DNS) produces **no span at all**, only a
log; `error.type` is recorded on the span only for status errors.

Fix: one client span per request covering send + body read (or a parent span
with `parse_response` as a child), with `otel.kind = "client"`,
`http.request.method`, `url.full`, `server.address`, `server.port`,
`http.response.status_code`, `error.type`. Same for `wreq` sends that are not
already covered by a fetch span.

### H4 (medium) — task-lifetime spans are children of short-lived spans

`alert_scheduler`, `alert_delivery`, `currency_refresh`, `rink_actor` and
`video_category_refresh` are created at `tokio::spawn` inside `loaded`
(`task.rs:54`), so they are children of a span that ends seconds later.
`download_manager` is created on the first mirror request, inside a plugin
handler, and lives for the process. `irc_outgoing` is a child of `run`.

Cost: these spans are never exported until shutdown (they never end); they
outlive their parents, so the waterfall shows a parent ending before a child;
the span objects are retained for the process lifetime; a crash loses them.

Fix: no span whose lifetime is a task. Create a span per unit of work instead:
per alert delivery, per cache-refresh tick, per actor request, per download.
Keep task lifecycle logs in short spans around the start/stop events.

### H5 (medium) — async fan-out children outlive their parents

`fetch_page_metadata` (`titles/lib.rs:164`) and `unwall_article`
(`unwall/lib.rs:415`) are created inside the handler and instrument the spawned
task, so they are children of `handle_event`, which returns immediately.
`download_media` is a child of the long-lived manager.

Cost: message traces inherit the duration of a network fetch (or a download);
the parent ends before the child; the trace is not a bounded unit of work.

Fix: capture the handler context when spawning/submitting and open a **new root
span with a link** to it (messaging semconv producer/consumer model). Message
traces stay short; the fetch/download trace is independently inspectable.

### H6 (medium) — external dependencies without spans

- S3: `object_exists` (`s3.rs:165`) and `upload_file` (`s3.rs:195`) have no
  spans; they are inside the handler (HEAD) or the manager (PUT).
- DNS: `plugins-dig` and `plugins-geoip` resolve through hickory with only a
  log (`plugins-geoip/src/lib.rs:207`).
- `wreq` sends outside the titles fetch span (instagram, chaturbate) have no
  dedicated client span.
- The IRC connect/handshake (`Client::from_config`, `identify`, `stream`) has
  no span under `run`.

Cost: no latency breakdown and no `error.type` span for the dependency; a slow
S3 or DNS lookup is only visible as "the handler took long".

Fix: add short client spans (`aws.s3.object_exists`, `aws.s3.upload`,
`dns.lookup`, `http.client`) with the semconv attributes. The DB already does
this correctly through `instrument_database`.

### H7 (low) — startup/shutdown parentage is ad hoc

`loaded` and `shutdown` are roots. `shutdown` per plugin has no parent; the
`runtime.shutdown` drain is not instrumented.

Fix: keep `loaded` as short roots (or link them to `startup`), and add one
`shutdown` span around `runtime.shutdown`, with the per-plugin shutdown spans
linked to it if the context can be propagated.

### H8 (low) — empty traces for non-event lines

`dispatch` is created before the command match (`zeta.rs:119`), so `PING`,
`PONG`, numerics, `MODE` and `CAP` produce a span and a trace with no children.

Fix: create the span only for the commands `EventIndex` routes (`PRIVMSG`,
`NOTICE`, `JOIN`, `PART`, `QUIT`, `NICK`, `KICK`, `Raw`), and add
`irc.command` and `irc.channel` fields.

### H9 (low) — span kind and names

`parse_response` and `fetch_page_metadata` are HTTP client operations but are
INTERNAL. Default `#[instrument]` names collide across services (`load` in
filter, notification and alert; `title`/`person`/`search`/`video`/`submission`
in the client crates) and say nothing about the subject (`process_url`,
`single_search`, `pending_for`).

Fix: `otel.kind = "client"` on HTTP spans; rename per §4 (Span naming).

---

## 3. Target hierarchy

```
zeta.startup                    root, short (config → DB → registry)
├── zeta.plugin.load            one per plugin, short
└── (zeta.startup ends here)

per event-bearing IRC line:
irc.dispatch                    root, short (ends at enqueue)
│                               irc.command, irc.channel, irc.sender.nick
└── zeta.event.handle           consumer, short
    │                           zeta.plugin.name, zeta.event.kind, irc.channel
    ├── plugin service spans    (ofn.record_url, imdb.search, filter.load, …)
    │   ├── db spans            (SELECT filters, INSERT url_records, …)
    │   └── HTTP spans          GET /v0/define, otel.kind=client
    └── spawned work            new trace, link to zeta.event.handle
        └── titles.fetch / unwall.resolve / media.download

zeta.shutdown                   root, short
└── zeta.plugin.shutdown        child or link

background (no task-lifetime spans):
alert.deliver                   per alert
alert.schedule                  per sync/due batch
currency.refresh                per tick
video_category.refresh          per tick
rink.eval                       per actor request
media.download                  per download, link to the requesting message
```

Rules:

- A span starts and ends around one unit of work. Task and process lifetimes
  are logs/resources, not spans.
- Synchronous work stays in the message trace: `irc.dispatch` →
  `zeta.event.handle` → service → I/O.
- Asynchronous work gets a new trace with a link to the handler context
  (producer/consumer). Do not keep the message trace open for it.
- No span for pure computation (routing, parsing, filter matching, argument
  parsing, formatting).

---

## 4. Span naming

Rules (OTel span naming + the internal-operation convention used by the
semconv-adjacent vendors):

- **One operation, one name.** `{verb} {object}` for generic operations, or
  `{domain}.{operation}` for app/plugin operations (`imdb.search`,
  `filter.load`, `media.download`).
- **Low cardinality.** No URL, id, nick, channel, show name, or error string in
  the name. Those are attributes. Use `otel.name` only with a static value.
- **Semconv where it exists.** HTTP: `{method} {route}` when a static route is
  available, `{method}` otherwise (MUST NOT default to the URI path). DB:
  `{db.operation.name} {target}` — the existing `SELECT filters` names are
  already correct.
- **No process/task lifetimes.** No `run`, `download_manager`,
  `alert_scheduler`, `rink_actor`. Those become per-unit spans (§3).
- **No bare function names.** A function name only works when it is unique and
  descriptive within the service; `load`, `create`, `search`, `title`,
  `person`, `video`, `submission` are not.
- **Name the subject, not the implementation.** `ofn.record_url` beats
  `process_url`; `titles.fetch` beats `fetch_page_metadata`; `unwall.resolve`
  beats `unwall_article`.

### 4.1 Explicit spans

| Current | Site | Proposed | Why |
|---|---|---|---|
| `zeta.startup` | `main.rs:32` | `zeta.startup` | domain-first; short |
| `run` | `zeta.rs:268` | *(removed)* | process lifetime |
| `irc_outgoing` | `zeta.rs:95` | *(removed)*; `irc.outgoing.error` on failure | task lifetime |
| `dispatch` | `zeta.rs:119` | `irc.dispatch` | root per event-bearing line |
| `second_signal` | `zeta.rs:159` | `zeta.signal.second` | short, on signal only |
| `loaded` | `task.rs:54` | `zeta.plugin.load` | explicit object |
| `handle_event` | `task.rs:70` | `zeta.event.handle` | event kind is an attribute |
| `shutdown` | `task.rs:81` | `zeta.plugin.shutdown` | explicit object |
| `download_manager` | `manager.rs:138` | *(removed)*; `media.manager.start/stop` if needed | task lifetime |
| `download_media` | `manager.rs:273` | `media.download` | per download |
| `alert_scheduler` | `alert/service.rs:143` | *(removed)*; `alert.schedule` per tick | task lifetime |
| `alert_delivery` | `alert/lib.rs:154` | *(removed)*; `alert.deliver` per alert | task lifetime |
| `currency_refresh` | `rink/lib.rs:256` | *(removed)*; `currency.refresh` per tick | task lifetime |
| `rink_actor` | `rink/actor.rs:61` | *(removed)*; `rink.eval` per request | task lifetime |
| `video_category_refresh` | `youtube/lib.rs:397` | *(removed)*; `video_category.refresh` per tick | task lifetime |
| `fetch_page_metadata` | `titles/lib.rs:164` | `titles.fetch` | plugin operation; URL in `url.full` |
| `unwall_article` | `unwall/lib.rs:415` | `unwall.resolve` | matches the service method |
| `parse_response` | `response.rs:86` | `{method} {route}` or `{method}` | HTTP semconv; move to cover the request |

### 4.2 `#[instrument]` spans (default function names)

| Current | Site | Proposed |
|---|---|---|
| `load` | `filter/service.rs:46` | `filter.load` |
| `add` | `filter/service.rs:66` | `filter.add` |
| `delete_ids` | `filter/service.rs:86` | `filter.delete` |
| `load` | `notification/service.rs:52` | `notification.load` |
| `create` | `notification/service.rs:81` | `notification.create` |
| `delete_all` | `notification/service.rs:160` | `notification.delete` |
| `load` | `alert/service.rs:109` | `alert.load` |
| `create` | `alert/service.rs:120` | `alert.create` |
| `pending_for` | `alert/service.rs:130` | `alert.pending` |
| `load` | `unwall/service.rs:64` | `unwall.load` |
| `add_sites` | `unwall/service.rs:160` | `unwall.site.add` |
| `remove_matching` | `unwall/service.rs:176` | `unwall.site.remove` |
| `statistics` | `unwall/service.rs:210` | `unwall.statistics` |
| `host_statistics` | `unwall/service.rs:220` | `unwall.host.statistics` |
| `process_url` | `ofn/lib.rs:49` | `ofn.record_url` |
| `title` | `imdb/client.rs:142` | `imdb.title` |
| `person` | `imdb/client.rs:159` | `imdb.person` |
| `search` | `imdb/client.rs:178` | `imdb.search` |
| `single_search` | `tvmaze/lib.rs:157` | `tvmaze.search` |
| `request_access_token` | `reddit/client.rs:171` | `reddit.auth` |
| `submission` | `reddit/client.rs:264` | `reddit.submission` |
| `video` | `reddit/client.rs:294` | `reddit.video` |
| `subreddit_about_info` | `reddit/client.rs:316` | `reddit.subreddit` |

### 4.3 DB and HTTP

- DB: keep `SELECT filters`, `INSERT url_records`, `DELETE notifications`, and
  the `SELECT … statistics` summaries. They follow `{operation} {target}` and
  are low cardinality.
- HTTP: the shared client currently names its span `parse_response`, a noun
  that describes only the body read. Move the span to cover the request
  (`send` + body) and name it `{method} {route}`. The API plugins know their
  routes (`GET /v0/define`, `GET /geo/1.0/direct`, `POST /execute`,
  `GET /maps/api/place/textsearch/json`); thread a static `route`/`name`
  parameter through `get_json`/`get_text`, or set `otel.name` at the call site.
  For arbitrary URLs (titles, instagram, chaturbate) use the method alone and
  keep the URL in `url.full`/`server.address`.
- `otel.kind`: `client` for HTTP/S3/DNS, `consumer` for `zeta.event.handle`,
  `server`/`internal` for `irc.dispatch`, `internal` for plugin operations.

### 4.4 Do not

- `run`, `download_manager`, `alert_scheduler`, `alert_delivery`,
  `currency_refresh`, `rink_actor`, `video_category_refresh` as span names.
- `load`, `create`, `search`, `title`, `person`, `video`, `submission`,
  `process_url`, `single_search`, `pending_for`, `fetch_page_metadata`,
  `unwall_article`, `parse_response` as bare names.
- Dynamic names (`GET https://…`, `fetch <url>`, `handle <id>`).
- Per-iteration spans (`process item`), per-function spans, or a span around
  pure computation.

### 4.5 Acceptance

- No two unrelated operations share a span name (`load` ×4 → 0).
- Span names contain no `http`, `https`, `#channel`, nick, id, or error text.
- Every HTTP span name starts with its method; every DB span name with its
  operation.
- `rg -n '#\[instrument' crates` shows an explicit `name =` on every generic
  service/repository/client method.

---

## 5. Log levels

Counts in production paths (tests excluded): ≈128 `debug`, 57 `warn`, 37
`error`, 37 `trace`, 7 `info`. The distribution is inverted: `error` is the
third-most-used level and most of those sites are expected, handled failures;
`info` is almost unused for per-operation events but used for three; and the
highest-frequency `debug` sites are enabled by the default filter.

Basis: the six-level synthesis in [Mendes & Petrillo, 2021] (Trace/Debug/Info/
Warn/Error/Fatal, with Error = unexpected behavior that must be investigated),
the production guidance from [.NET], [AWS], and [Better Stack] (Information is
the production default; Debug/Trace off; Error means an unhandled failure, not
a user error), the [OTel logs severity model] (ERROR and above mean "erroneous
situation"), the [OTel exceptions severity guidance] (WARN for handled client
failures, ERROR for unhandled), and the ["if it logs, it does not rethrow"]
rule. `tracing` has no Fatal; `error` is the highest level available, and a
fatal condition is `error` + process exit.

### 5.1 Policy

| Level | Use for | Examples | Never |
|---|---|---|---|
| `error` | Unhandled failure of the operation, data loss, broken component; the span should be marked failed. `tracing` has no Fatal, so fatal conditions are `error` + exit | DB write lost; alert lost; panic; invalid signature/config that disables a feature; write path stopped | Expected API 4xx, user input, not found, a retry that will happen |
| `warn` | Unexpected but handled; degraded; the operation succeeded or will be retried. Also config typos and a feature disabled while the bot continues | transport timeout that is replied to, stale cache kept, unknown config key, plugin failed to load, delivery restored for retry | per-message/per-URL noise; an expected user mistake |
| `info` | Lifecycle and state changes | tracing initialized, plugins registered/disabled, shutting down, migrations | per command, per URL, per message |
| `debug` | Diagnostic detail for one operation | per-URL processing, cache decisions, plugin task start/stop, per-file upload; expected user input and not-found results | per IRC line payload, per progress tick |
| `trace` | Very high-frequency detail; only when tracing one path | per-filter match, per DB row/query, per progress update, raw IRC payload | anything wanted in normal production |

A user mistake or a miss is **info/debug**, not warn or error: "if an error is
expected and handled, such as a user entering a bad value, it is not an error
level event" [Safeguard]. Warn is for *unexpected* but non-fatal conditions, not
for every failure a caller recovers from.

Cross-cutting rules:

1. **One failure, one line.** The layer that has the unique context owns the
   line: HTTP status/body in `zeta-http`, operation context in the plugin,
   unexpected errors in the host. "If the code rethrows, it should not log. If
   it logs, it should not rethrow" [Safeguard]. If a handler logs an error it
   handles it and returns `Ok(())`; a propagated error is by definition
   unhandled, so the host logs it once at error. Client crates do not log; they
   return redacted errors.
2. **The level sets the span status.** `tracing-opentelemetry` turns any
   `error!` event into `Status::Error` on the current span, and the OTel logs
   model treats ERROR and above as "an erroneous situation". A handled
   404/rate-limit logged at `error` makes a healthy handler span red, so the
   level must match whether the operation actually failed.
3. **Levels are a volume/actionability filter, not the data model.** The detail
   belongs in span attributes and event fields; levels only decide what is
   emitted and what is marked failed. Do not add custom levels (NOTICE, FATAL,
   SECURITY); six is the practical ceiling [Mendes & Petrillo].


### 5.2 Findings

**L1 (high) — `error!` for expected, handled failures.**

- `crates/zeta-http/src/response.rs:57-58` logs every non-404 non-success at
  error, including 400/401/403/429. These are routine API outcomes (invalid
  user query, rate limit).
- `crates/zeta-http/src/client.rs:41` logs every transport failure (timeout,
  refused) at error; the caller usually replies and continues.
- The client crates' `request_error` (`reddit/src/client.rs:29`,
  `kagi/src/client.rs:29`, `coinmarketcap/src/client.rs:251`,
  `dendanskeordbog/src/client.rs:21`) do the same.
- `crates/plugins-{reddit,tiktok,instagram}/src/lib.rs` log a handled URL
  failure at error (`could not process url: …`).
- `crates/zeta-mirror/src/manager.rs:233` (unsafe id),
  `:321` (download failed), `:370` (temp-dir cleanup) log expected failures at
  error.
- `crates/zeta-plugin-runtime/src/task.rs:67` is only wrong if handlers
  propagate expected failures. A propagated error is by definition unhandled by
  the plugin, so `error` is the correct level; the fix is for handlers to handle
  expected failures and return `Ok(())`. The same applies to `#[instrument(err)]`
  on service methods: it emits an error event and marks the span Error even for
  a handled miss. Use `#[instrument(err(level = Level::DEBUG))]` (or `WARN`)
  where the caller handles the error.

**L2 (high) — one failure is logged more than once.**

`crates/reddit/src/client.rs:29` (client) → `crates/plugins-reddit/src/lib.rs:103`
(handler) is two lines for one failed request; the handler discards the result,
so the host does not add a third. Spotify/`zeta-http` paths add the host line
(`task.rs:67`) when the handler returns `Err`, and the client logs the same
failure first. The same pattern exists for Kagi and CoinMarketCap. The
`parse_response` comment already acknowledges this for status errors ("avoids
logging one failure three times over"), but the ownership rule is not applied
consistently.

**L3 (medium) — `info!` for per-operation events.**

- `crates/plugins-geoip/src/lib.rs:258` `info!(ip = %info.ip, "resolved")` —
  per command.
- `crates/plugins-github/src/lib.rs:101` `info!(query, %channel, "searching
  github")` — per command.
- `crates/reddit/src/client.rs:234` `info!(%err, "resource not found")` — a 404,
  expected.
Keep the lifecycle infos (`telemetry.rs:127`, `zeta.rs:214,304`,
`plugins/src/lib.rs:224`).

**L4 (medium) — high-frequency `debug!` that is on by default.**

- `crates/zeta/src/zeta.rs:131` `debug!(payload = %message, …)` — every IRC
  line.
- `crates/zeta-mirror/src/manager.rs:302` `debug!(… percent … ?progress …)` —
  every yt-dlp progress update; `zeta_mirror=debug` is in the default filter.
- `crates/zeta-mirror/src/s3.rs:166` (per HEAD) and `:196` (per file upload) are
  per URL/file but on by default; the upload pair is useful at debug, the HEAD
  is trace.
- Plugin per-URL `?show`/`?dossier`/`?video`/`?origin` dumps are off by default,
  but become a flood with `plugins=debug`; they belong at trace.

**L5 (medium) — fatal conditions at `warn`.**

- `crates/zeta/src/zeta.rs:92,314,315` write path failed/stopped — the bot
  cannot send; this is fatal (error).
- `crates/zeta-plugin-runtime/src/runtime.rs:123` a plugin task panicked —
  error.
- `crates/zeta-mirror/src/handle.rs:98` invalid public URL base disables
  mirroring — error.
- `crates/zeta-mirror/src/lib.rs:253` download manager not running — error.
- `crates/plugins-notification/src/lib.rs:164` delivery failed but the
  notification is restored for retry — warn, not error.
- `crates/plugins-howlongtobeat/src/lib.rs:288` token expired and is refreshed —
  debug (expected).
- `crates/plugins-coinmarketcap/src/lib.rs:371` is already correctly
  classified (user errors are not logged); apply that pattern to the other API
  plugins.

**L6 (low) — trace/debug inversions.**

- `crates/reddit/src/client.rs:214` `trace!("response is ok, parsing
  response")` — debug (per API call, useful).
- `crates/plugins-alert/src/lib.rs:142`, `service.rs:187,298` — per alert;
  debug is more useful than trace (alerts are rare).
- `crates/plugins-notification/src/service.rs:141` restore — debug.
- The DB repository `trace!` lines duplicate the `instrument_database` spans;
  keep them at trace or remove them.

**L7 (medium) — levels and span status.**

Because `error!` marks the current span Error, the ERROR/WARN decision is also
the trace-status decision (see L1). Expected 404/rate-limit paths should be
`debug`/`warn` so the handler span remains successful; only data loss and bugs
should paint it red.

### 5.3 Representative changes

| Site | Current | Proposed |
|---|---|---|
| `zeta-http/response.rs:57` | error for all non-success | 5xx error, 4xx warn, 404 debug |
| `zeta-http/client.rs:41` | error | warn |
| `reddit/kagi/coinmarketcap/dendanskeordbog request_error` | error | remove; caller logs once |
| `zeta-plugin-runtime/task.rs:67` | error | keep error for unhandled; handlers return `Ok` for expected failures; use `err(level = WARN/DEBUG)` on handled service errors |
| `plugins-reddit:103,142`; `plugins-tiktok:117`; `plugins-instagram:220` | error | warn for transport/5xx, debug for not-found/user errors |
| `plugins-notification:164` | error | warn (restored for retry) |
| `zeta-mirror/manager.rs:233` | error | warn (invalid input) |
| `zeta-mirror/manager.rs:321` | error | warn (expected download failure) |
| `zeta-mirror/manager.rs:370` | error | warn (cleanup) |
| `zeta/src/zeta.rs:92,314,315` | warn | error (fatal write path) |
| `zeta-plugin-runtime/runtime.rs:123` | warn | error (panic) |
| `plugins-geoip:258`, `plugins-github:101`, `reddit:234` | info | debug |
| `zeta/src/zeta.rs:131` | debug | trace |
| `zeta-mirror/manager.rs:302` | debug | trace |
| `plugins-howlongtobeat:288` | warn | debug |

---

## 6. Work plan

Ordered by dependency. The performance fixes (inline I/O, mirror uploads) are a
prerequisite for Phase 1: parenting a 15-minute handler span to the message
trace would make message traces long again.

### Phase 0 — stop the bleeding (independent)

**P0.1 — Scope `startup`, delete `run`**

- `crates/zeta/src/main.rs:32-63`: instrument only the config/DB/`Zeta::new`
  future; call `zeta.run().await` outside the span.
- `crates/zeta/src/zeta.rs:268`: remove `#[tracing::instrument(name = "run")]`.
- Acceptance: no span named `run`; `zeta.startup` duration is the startup cost
  (config + DB + registration), not the process lifetime.

**P0.2 — HTTP client span covers the request**

- `crates/zeta-http/src/client.rs:27`: wrap `send` (or `get_json`/`get_text`)
  in a span created before the request, with `otel.kind = "client"`,
  `http.request.method` (from the `RequestBuilder`), `url.full` (redacted),
  `server.address`, `server.port`.
- `crates/zeta-http/src/response.rs:86`: either drop the `parse_response`
  span and set status/`error.type` on the client span, or keep it as a child.
- `crates/zeta-http/src/emulated.rs:15`: same for `wreq` where the caller
  doesn't already own a span (instagram, chaturbate); titles' fetch span can
  remain the parent.
- Acceptance: a wiremock test asserts one span with `otel.kind=client`,
  `http.request.method`, status and duration covering the request; a refused
  connection produces a span with `error.type` (not only a log).

### Phase 1 — connect the message trace (depends on the inline-I/O fix)

**P1.1 — Mailbox envelope with the handler span**

- `Subscriber::send` (`dispatch.rs`): create `zeta.event.handle` while the
  `irc.dispatch` span is current, with `otel.kind = "consumer"`,
  `zeta.plugin.name`, `zeta.event.kind`, `irc.channel`, and `.or_current()`.
- Mailbox item becomes `Envelope { event, span }`
  (`dispatch.rs`/`runtime.rs`); `task.rs` runs the handler with
  `.instrument(span)`.
- Rationale: `Instrument` enters the span every time the future is polled and
  exits it when the future yields (`tracing::Span::enter` must not be held
  across `.await`); `Span` is `Send + Sync`, so it can cross the task boundary.
  `#[instrument]` on the dispatch functions only instruments the routing
  future, not the plugin task, so it cannot parent the handler.
- Trade-off: the span starts at enqueue, so it includes the time the event waits
  in the mailbox. If handler-only timing is wanted, capture
  `Span::current().context()` in the dispatcher and use
  `OpenTelemetrySpanExt::set_parent` in the task instead (requires the
  opentelemetry deps; the parent may end before the child).
- Acceptance: one trace per PRIVMSG with N `zeta.event.handle` children; zero
  root `zeta.event.handle` spans; handler logs still visible in OTLP.

**P1.2 — Remove task-lifetime spans**

- `alert/service.rs:143`: remove `alert_scheduler` from the spawn; open a short
  span per tick (only when there is work) or per delivered batch.
- `alert/lib.rs:154`: remove `alert_delivery`; open `alert.deliver` per alert.
- `rink/lib.rs:256`: remove `currency_refresh`; open
  `currency_refresh.tick` per tick.
- `rink/actor.rs:61`: remove `rink_actor`; open `rink.eval` per request.
- `youtube/lib.rs:397`: remove `video_category_refresh`; open a tick span.
- `zeta-mirror/manager.rs:138`: remove `download_manager`; keep/log lifecycle
  in short spans around start/stop.
- `zeta/src/zeta.rs:95`: remove `irc_outgoing`; on failure, log inside a short
  `irc_outgoing` span so the OTel layer keeps the event.
- Acceptance: no span older than the unit it measures; no child of `loaded`
  with a longer lifetime than `loaded`.

### Phase 2 — async fan-out and missing dependencies

**P2.1 — Link spawned work to the message**

- `titles/lib.rs:166`, `unwall/lib.rs:417`: create the fetch/article span as a
  new root and `follows_from` the handler span; `tracing` maps `follows_from`
  to an OpenTelemetry link, and the followed span may close first.
- `zeta-mirror/src/manager.rs:47` (`DownloadRequest`): carry the producer
  context; `media.download` is a new root with a link to it.
- Acceptance: message traces end when handling ends; each fetch/download trace
  has a link to the message.

**P2.2 — S3 and DNS client spans**

- `zeta-mirror/src/s3.rs:165,195`: add `aws.s3.object_exists` /
  `aws.s3.upload` spans, `otel.kind=client`, `aws.s3.key`,
  `server.address`, `http.request.method`, status/`error.type`.
- `plugins-geoip/src/lib.rs:207`, `plugins-dig`: add `dns.lookup` around the
  resolver call (`dns.question.name`).
- Acceptance: latency of a mirror HEAD/PUT and a DNS lookup is visible in the
  trace; failures carry `error.type` on a span.

**P2.3 — Shutdown and startup parentage**

- Add a `zeta.shutdown` span around `runtime.shutdown` (`zeta.rs:338`); link the
  per-plugin `zeta.plugin.shutdown` spans to it.
- Optionally link `loaded` spans to `startup`.
- Acceptance: one shutdown trace with per-plugin children/links.

### Phase 3 — names, levels and configuration

**P3.1 — Attribute convention** (from the attribute review)

- `zeta.plugin.name` instead of `plugin`; `zeta.event.kind`; `irc.channel`.
- `error`/`err`/`e` → `exception.type` + `exception.message`; `error.type`
  stays the low-cardinality class; `_OTHER` instead of `other`.
- `media.id`, `imdb.title.id`, `alert.id`, … instead of `id`/`media_id`.
- `file.path`, `aws.s3.key`, `process.command`, `geo.location.lat/lon`,
  `search.query` instead of the bare names.
- Static event names; values in fields (no `{value}` in tracing messages).
- `zeta.plugin.load`/`zeta.plugin.shutdown` get the same conventions.
- Acceptance: `rg` finds no bare `plugin`, `id`, `count`, `query`, `name`,
  `path`, `key`, `body`, `channel`, `error`, `err`, `e` in tracing fields
  except the documented exceptions.

**P3.2 — Default filter**

- `crates/zeta/src/telemetry.rs:115-119`: `zeta=info` by default (or explicit
  module directives); `zeta.event.handle` becomes `debug_span!` if per-event
  spans should be opt-in. Decide explicitly whether handler logs stay in OTLP
  when the span is off (they do not; they remain stdout).
- Depends on P3.5: the filter and the levels must land together, otherwise
  `zeta=info` hides the `debug` lines that should have been `trace`, and the
  `debug` lines that should have been `info` stay hidden.
- Acceptance: default config does not create per-event spans or the per-message
  payload debug line; `RUST_LOG=zeta=debug` restores them.

**P3.3 — Resource attributes**

- Add `deployment.environment.name` (`development`/`staging`/`production`/
  `test`) in `telemetry.rs`.
- Acceptance: service list shows `zeta` with an environment.

**P3.4 — Span names**

- Rename per §4.1 and §4.2. Every generic service/client method gets an
  explicit `#[instrument(name = "...")]`; the task-lifetime spans are removed
  in P1.2 rather than renamed.
- Thread the static route (or `otel.name`) into `send`/`get_json`/`get_text` so
  HTTP spans are `{method} {route}` (or `{method}` for arbitrary URLs) instead
  of `parse_response`.
- Acceptance: §4.5.

**P3.5 — Log levels and ownership**

- Apply the policy in §5.1 and the high-impact set in §5.3.
- Remove logging from the client crates' `request_error`; return the redacted
  error and let the caller log once.
- Remove `zeta-http::send`'s error log; the span carries `error.type` and the
  caller owns the operation line.
- Classify HTTP status errors: 5xx `error`, 4xx `warn`, 404 `debug`.
- Downgrade the default-enabled per-line/per-progress logs to `trace`; upgrade
  fatal write-path/panic/disabled-feature logs to `error`.
- Classify handled delivery failures (`notification`) as `warn`; keep data-loss
  failures (`ofn` insert, alert delivery, upload) at `error`.
- Use `#[instrument(err(level = Level::DEBUG))]` or `WARN` for service methods
  whose errors are handled by the caller (a miss, a user error); leave the
  default `ERROR` for unexpected failures.
- Treat `trace`-level payloads as sensitive: the raw IRC line, request/response
  bodies, and Debug dumps must stay off in production by default (AWS/Better
  Stack both call this out). Do not add custom levels; six is the ceiling.
- Acceptance: `error` is reserved for unhandled/data-loss/fatal/bugs (no
  expected 4xx, user input, or not-found at error); no `info!` on per-command
  paths; no `debug!` per IRC line or progress tick; each failure path logs once
  (manual review of the reddit/kagi/coinmarketcap/zeta-http error paths); a
  4xx-only handler span is not marked Error.

### Phase 4 — guardrails

**P4.1 — Hierarchy test**

Add a test layer (in `zeta-plugin-runtime` tests) that records span IDs,
parent IDs and names. Drive `EventIndex::dispatch` + a `PluginTask` with one
event; assert:

- exactly one root span (`irc.dispatch`);
- `zeta.event.handle`'s parent is the `irc.dispatch` span;
- no `zeta.event.handle` root.

**P4.2 — Trace-shape checks in review**

- Jaeger/Tempo: max span duration over 1h (expect < 60s except downloads);
- roots per PRIVMSG (expect 1);
- `zeta.event.handle` without a parent (expect 0);
- failed HTTP requests with a span (expect 100%);
- span names shared by unrelated operations (`load`) (expect 0);
- span names containing a URL, channel, nick or id (expect 0);
- handler spans marked Error for a 4xx-only path (expect 0);
- duplicate lines for one failure (expect 1 per failure, by manual review).

**P4.3 — Documentation**

- Add the attribute convention and the "no task-lifetime spans" rule to
  `AGENTS.md` (or link this file).
- Note the `url.full` redaction deviation from the HTTP semconv.

---

## 7. Open questions

1. Parent vs link for `zeta.event.handle`: the plan uses a parent for one trace
   per message. If the handler becomes long (inline I/O), use a link and a new
   trace instead. Decide after the inline-I/O work lands.
2. `zeta.*` vs generic names (`plugin.name`, `media.id`): the plan uses `zeta.*`
   for host internals; revisit if these attributes ever become useful outside
   zeta.
3. `dispatch` only for event-bearing lines: confirm the full set of routed
   commands (including `Raw`) so `MODE`/`CAP`/numerics don't regress.
4. Log bridge: `tracing-opentelemetry` exports handler logs as span events.
   Since span events are being deprecated, decide later whether to add
   `opentelemetry-appender-tracing` (new dependency) or keep span events until
   the bridge updates.

---

## 8. Live verification (2026-10-30)

Run against the local InspIRCd and Jaeger from `compose.yaml`, with a minimal
build (`dig`, `titles`, `help`) and `RUST_LOG=zeta=debug,plugins=debug`. Three
lines were sent to `#zeta.dev`: `.dig example.com`, `https://example.com`, and
a plain message; the final run repeated the `.dig` line. After a graceful
shutdown, the Jaeger v3 query API returned:

- 15-20 spans and 13-16 traces per run; no span over 60 s.
- `zeta.event.handle` is never a root (0/N) and is parented to `irc.dispatch`.
- `dns.lookup` is a child of the dig handler; `titles.fetch` is a new root with
  one link, and its `GET` client span is its child.
- Each `zeta.plugin.shutdown` span is a root with one link to the
  `zeta.shutdown` span (the hooks run in their own tasks, so a link, not a
  child).
- Names observed: `GET`, `dns.lookup`, `irc.dispatch`, `titles.fetch`,
  `zeta.event.handle`, `zeta.plugin.load`, `zeta.plugin.shutdown`,
  `zeta.shutdown`, `zeta.startup`; no URL, channel, nick or id in a name.
- Resource attributes include `service.name=zeta`, `service.version=0.14.0`
  and `deployment.environment.name=test`.

After the H8 fix, `irc.dispatch` roots correspond to routed commands only
(PRIVMSG/NOTICE/JOIN/PART/QUIT/NICK/KICK/Raw) plus `ERROR`; server numerics and
PING/PONG produce no trace.

## 9. References

Span hierarchy and naming:

- [OTel — Naming](https://opentelemetry.io/docs/specs/semconv/general/naming/)
- [OTel — Messaging spans](https://opentelemetry.io/docs/specs/semconv/messaging/messaging-spans/)
- [OTel — HTTP spans](https://opentelemetry.io/docs/specs/semconv/http/http-spans/)
- [OTel — Sampling](https://opentelemetry.io/docs/concepts/sampling/)
- [Embrace — Handling long-running spans](https://embrace.io/blog/opentelemetry-challenges-handling-long-running-spans/)
- [Ian Duncan — Phantom spans](https://www.iankduncan.com/articles/2023-08-28-opentelemetry-gotchas-phantom-spans)
- [Honeycomb — Span or attribute](https://www.honeycomb.io/blog/span-or-attribute-opentelemetry-custom-instrumentation)
- [Webstack — Tracing span design](https://www.webstackbuilders.com/articles/opentelemetry-span-design-granularity-overhead)
- [Grafana Tempo — Best practices for traces](https://grafana.com/docs/tempo/latest/set-up-for-tracing/instrument-send/best-practices/)

Log levels:

- [Mendes & Petrillo — Log severity levels matter: A multivocal mapping (2021)](https://arxiv.org/abs/2109.01192)
- [Microsoft — Logging in C#: log levels](https://learn.microsoft.com/en-us/dotnet/core/extensions/logging?tabs=command-line)
- [AWS — Logging best practices](https://docs.aws.amazon.com/prescriptive-guidance/latest/logging-monitoring-for-application-owners/logging-best-practices.html)
- [Better Stack — Log levels explained](https://betterstack.com/community/guides/logging/log-levels-explained/)
- [Google Cloud — LogEntry severity](https://cloud.google.com/logging/docs/reference/v2/rest/v2/LogEntry#LogSeverity)
- [OTel — Logs data model (SeverityNumber)](https://opentelemetry.io/docs/specs/otel/logs/data-model/)
- [OTel — Exceptions in logs](https://opentelemetry.io/docs/specs/semconv/exceptions/exceptions-logs/)
- [Safeguard — Log an error once](https://safeguard.sh/resources/blog/log-an-error-once-avoid-duplicate-stack-traces)
- [Charity Majors — Logs vs structured events](https://charity.wtf/p/logs-vs-structured-events)
- [The Twelve-Factor App — Logs](https://12factor.net/logs)
