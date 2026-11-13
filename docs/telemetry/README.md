# Telemetry

Zeta exports **spans** over OTLP and writes structured JSON logs to stdout. It has no custom
metrics and no span events. Events emitted outside a span are dropped by the OpenTelemetry
layer, so any background work that should be observable opens its own span.

This directory is the source of truth for span names, attribute names, and log levels. Change
the code and the matching doc in the same PR. The complete attribute list is in
[attributes.md](attributes.md).

## Priority order

1. **OpenTelemetry semantic conventions.** Use the exact key, type, and value from the
   [naming rules](https://opentelemetry.io/docs/specs/semconv/general/naming/) and the
   [attribute registry](https://opentelemetry.io/docs/specs/semconv/registry/attributes/).
2. **Elastic Common Schema only where OTel has no equivalent.** Never use an ECS name where
   OTel defines one (`network.protocol.name`, `source.address`, `url.*`, `error.type`). No ECS
   attribute is currently emitted. See the [ECS event](https://www.elastic.co/docs/reference/ecs/ecs-event)
   and [network](https://www.elastic.co/docs/reference/ecs/ecs-network) fields.
3. **Our names.** App-wide keys go under `zeta.*`; a plugin owns its plugin name as a root
   namespace; `media.*` is the shared mirror domain; `irc.*` is the IRC namespace.

## Naming

- Lowercase, dot-separated namespaces, snake_case inside a component:
  `messaging.destination.name`, `media.download.progress_percent`.
- Singular for one entity, plural (and an array value) for several: `alert.id` vs `alert.ids`.
- Counts end in `.count`; percentages name the unit: `media.download.progress_percent`.
- Never use a bare key. The banned keys are `plugin`, `id`, `count`, `query`, `name`, `path`,
  `key`, `body`, `channel`, `error`, `err`, and `e`; they collide across domains.
- `otel.*` is reserved by OpenTelemetry. Only `otel.name` and `otel.kind` are used, and only as
  `tracing-opentelemetry` controls, not as exported attributes.
- Do not invent keys under an existing OTel namespace (`http.*`, `url.*`, `db.*`, …) for a
  different meaning.
- Keep values bounded. Never put a URL, channel, nick, query, or free-form text in an attribute
  *name*; values carry them only where the registry says so, and URLs always go through
  `RedactedUrl`/`RedactedUrlRef`.

## Spans

- One span per unit of work: an IRC line, an event delivery, an HTTP request, a DB query, a
  mirror, a plugin hook, a scheduler tick. No process- or task-lifetime spans (`run`,
  `*_scheduler`, `*_actor`).
- Name spans `{domain}.{operation}` (`imdb.search`, `alert.deliver`, `media.download`). Keep
  the name low-cardinality: never a URL, id, channel, nick, or free text.
- Repeated spans under one parent carry a bounded discriminator in `otel.name`
  (`zeta.event.handle dig command`), keeping the span name itself stable for filters and
  metrics.
- `otel.kind` sets the OTel span kind where the default is wrong: `client` for HTTP, S3, and
  database spans, `consumer` for inbound IRC.
- Async fan-out opens its child span while the handler span is current, then moves the span
  into the task (`titles.fetch`, `unwall.resolve`, `media.download`). The unit stays in the
  message's trace after the handler returns.
- Spans that parent HTTP/DNS/S3 client spans should set `target: "zeta.plugins"`. The host's
  `irc.dispatch` and the runtime's `zeta.event.handle` also use that target, so one filter
  decision (`zeta=info`) enables the whole message trace. The handler and async fan-out spans
  parent the client spans; `#[instrument]` spans inside a `plugins-*` crate keep their crate
  target, which `plugins=info` enables by default. The explicit target keeps a span enabled by
  the `zeta` directive even when a `RUST_LOG` filter disables the plugin's own crate, so its
  client children do not become trace roots.
- DB spans come from `#[instrument_database]`: name `{OPERATION} {collection}`
  (`SELECT filters`), with the `db.*` fields. Do not hand-write them.

Current core spans:

| Span | Unit | Notes |
| --- | --- | --- |
| `zeta.startup` | startup phases | DB connect/migrate and plugin construction in `main`; then the plugin load barrier in `run` |
| `irc.dispatch` | one routed incoming IRC line | root of a message trace; INFO; target `zeta.plugins`; consumer kind |
| `zeta.event.handle` | one event to one subscriber | INFO; target `zeta.plugins`; `otel.name` discriminator |
| `zeta.plugin.load`, `zeta.plugin.shutdown` | one lifecycle hook | INFO; `otel.name` includes the plugin |
| `zeta.shutdown` | plugin drain and shutdown hooks | INFO; parents the `zeta.plugin.shutdown` spans |
| `http.client` | one HTTP request | INFO; client kind |
| `dns.lookup` | one DNS query | INFO |
| `aws.s3.object_exists`, `aws.s3.upload` | one S3 operation | INFO; client kind |
| `media.download` | one mirror request (queue + download + upload) | INFO; child of the requester |
| `{OPERATION} {collection}` | one repository call | from `#[instrument_database]` |
| plugin `{domain}.{operation}` | one handler, fetch, or refresh | e.g. `imdb.search`, `titles.fetch`, `alert.deliver` |

## IRC messaging

Both the per-line `irc.dispatch` span and the per-subscriber `zeta.event.handle` span carry the
OTel messaging attributes. The handler span repeats the event's values so it is self-contained.

| Attribute | Value | Notes |
| --- | --- | --- |
| `messaging.system.name` | `irc` | always |
| `messaging.operation.name` | `privmsg`, `notice`, `join`, `part`, `quit`, `nick`, `kick`, … | lowercased IRC command; an unmodeled command is lowercased; `error` for server `ERROR` |
| `messaging.operation.type` | `process` | inbound only |
| `messaging.destination.name` | channel or nick | target of `PRIVMSG`, `NOTICE`, `JOIN`, `PART`, `KICK`; absent for `QUIT`, `NICK`, `RAW`, and numerics |
| `messaging.message.id` | IRCv3 `msgid` | read from any tagged event; the client requests the `message-tags` capability at registration, and the attribute is absent when the server sends none |

## Log levels and default filter

| Level | Use |
| --- | --- |
| `error` | unhandled failure, data loss, fatal startup or shutdown |
| `warn` | handled but unexpected: request failure, retry, dropped event, missing configuration |
| `debug` | per-operation detail and expected misses: 404s, user input errors, cache hits |
| `trace` | per-line payloads: `irc.raw`, download progress, deleted-id lists |

- One failure, one log line, at the layer with the unique context. Do not log the same error at
  every level up the stack.
- An `error!` event inside a span marks the span failed.
- The default filter is defined in `crates/zeta/src/telemetry.rs`:
  `zeta=info,plugins=info,reddit=debug,dendanskeordbog=debug,kagi=debug,coinmarketcap=debug,zeta_http::error=debug,zeta_http::json=debug`
  - `zeta=info` covers host/runtime spans and logs; `plugins=info` keeps plugin spans (and
    their HTTP/DNS/S3 children) in the export.
  - The client crates' request-failure and response-parse lines live in
    `zeta_http::error::request` and `zeta_http::json`, so exactly those two targets
    are enabled at debug rather than all of `zeta_http=debug`.
- Override with `RUST_LOG`, e.g. `RUST_LOG=info,plugins=debug,zeta=debug` for per-operation
  detail. `RUST_LOG` replaces the default filter entirely.

## Checklist: adding a span or attribute

1. Search the [OTel registry](https://opentelemetry.io/docs/specs/semconv/registry/attributes/)
   first; use the standard key, type, and value if one exists. Only then pick a namespace
   (the plugin's own name, `media.*`, or `zeta.*`).
2. Apply the naming rules: lowercase dots, snake_case, singular/plural, `.count` for counts, no
   bare keys.
3. Record stable values at span creation. Gate expensive extraction (channel lookup, IRCv3 tag
   scan, message-id) on `tracing::enabled!` so a filtered span pays nothing.
4. Put a per-plugin or per-event discriminator in `otel.name`; never put ids in the span name.
5. Set `target: "zeta.plugins"` if the span will parent HTTP/DNS/S3 client spans.
6. Record failures as `error.type` and `exception.message`; let the event level set the span
   status.
7. Add the key to [attributes.md](attributes.md) in the same PR.
8. Test the shape where it matters: dispatch spans in
   `crates/zeta-plugin-runtime/tests/dispatch_spans.rs`, HTTP spans in
   `crates/zeta-http/tests/client_spans.rs`.
