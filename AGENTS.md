# AGENTS.md

## Layout

IRC bot in Rust (edition 2024, workspace `rust-version = "1.99"`). Dependency direction: `zeta` → `plugins` → {`zeta-plugin`, infra, client crates}.

- `crates/zeta` — host binary: Figment config, database/migrations, tracing, IRC client loop; wires `zeta-plugin-runtime` to the registered plugins. CLI in `src/cli.rs`.
- `crates/plugins` — thin facade over the bundled `plugins-<name>` crates, one cargo feature each (`all` aggregates). Each plugin declares itself with `#[plugin]` (`zeta-plugin-macros`, re-exported by `zeta-plugin`); `inventory` collects registrations into `PluginsConfig` / `register_bundled_plugins`. The facade owns `BUNDLED_PLUGIN_NAMES`, the feature graph, and **all migrations** (`crates/plugins/migrations/`). `plugins-shared` holds helpers used by more than one plugin (`urls/`, mirror reply).
- `crates/zeta-plugin` — the plugin API (`Plugin<Context>`, events, `CommandSpec`, context, filtering, IRC reply helpers in `zeta_plugin::irc`, hostmasks, DB helpers). Never depends on zeta or plugins. There is no prelude: import from `zeta_plugin::{…}` and `zeta_plugin::irc::{…}`.
- `crates/zeta-plugin-runtime` — `Registry`, `PluginRuntime`, `PluginTask`, `EventIndex`, dispatch. Never depends on zeta or plugins.
- Infra: `zeta-http`, `zeta-cache`, `zeta-url`, `zeta-utils`, `zeta-mirror`; test helpers in `zeta-test-support` (re-exports `wiremock`).
- `zeta-unicode` (+ `-build` trainer, `-format` shared constants) — the Nomina Unicode image behind `string-utils`' `.uni`.
- Client crates `kagi`/`reddit`/`dendanskeordbog`/`coinmarketcap` — standalone; never depend on `zeta-plugin`. Single-consumer clients stay inside their plugin (e.g. `imdb`, `unwall`).
- Lints live in root `[workspace.lints]` (pedantic + nursery, `unsafe_code = forbid`, `missing_docs = warn`); new members set `[lints] workspace = true`. Brand names for docs go in `clippy.toml` `doc-valid-idents`.

## Verify

Cheapest first; each step catches something the others don't:

1. `cargo fmt --all --check`, `cargo sort --check --workspace` (no CI job — keep green locally).
2. `cargo clippy --workspace --all-targets --all-features --no-deps` — warning-free (CI also runs nightly).
3. `cargo test --workspace --all-features --no-fail-fast` — needs postgres (see sqlx).
   - `default-members = ["crates/zeta"]`: bare `cargo test`/`check`/`build` only touch the bot; pass `--workspace` or `-p`.
   - Plugin tests (incl. network tests) live in `plugins-<name>`; `-p zeta` silently matches none. Focused: `cargo test -p <crate> --all-features <filter>`; no DB: `cargo test -p <crate> --lib`.
   - CI uses `cargo nextest run --workspace --all-features` (one process per test) plus `cargo test --workspace --all-features --doc` (nextest skips doctests).
4. `RUSTDOCFLAGS=-D warnings cargo doc --workspace --no-deps --all-features` — broken intra-doc links.
5. Each plugin in isolation (`--all-features` unification hides missing deps):
   - Touched plugin: `cargo check -p zeta --no-default-features --features "plugins/<feature>,rustls,database"`.
   - Full sweep (CI `plugin-features.yaml`; run locally after touching shared crates):
     `for f in $(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "plugins") | .features.all[]'); do cargo check -p zeta --no-default-features --features "plugins/$f,rustls,database" || echo "FAIL $f"; done`

- CI also runs nightly with `-Zrandomize-layout`; zizmor audits `.github/workflows/**` changes (SHA-pinned actions, `persist-credentials: false`, explicit `permissions`).
- Default features pull `wreq`/BoringSSL (via `titles`/`instagram`) → needs `cmake` + `libclang-dev`. On header/clang-binding failures install packages; don't disable features or patch build scripts.
- Tests are inline `#[cfg(test)] mod tests`; fixtures in `crates/*/tests/fixtures/**`, loaded with `fixture!` (not hand-rolled `CARGO_MANIFEST_DIR`). Network tests are `#[ignore = "needs network access"]` → `cargo test -p plugins-<name> --all-features -- --ignored <name>`. Helpers in `zeta-test-support`: `test_context()` (lazy sqlx pool), `settings_tests!`, `assert_parses`, `not_found_server`, `refused_get`/`timeout_error` + `assert_redactions`/`assert_timeout_redactions`.
- Disk: BoringSSL + docs + per-feature builds fill disk fast. Use `cargo sweep --maxsize 8GiB` or `cargo clean -p <crate>`; never a bare workspace `cargo clean` in loops.

## sqlx

- Runtime query fns only (`sqlx::query`/`query_as`, `FromRow`) — no compile-time macros, so no `DATABASE_URL` to build.
- Migrations: `crates/plugins/migrations/<timestamp>_<name>.{up,down}.sql`, embedded by `crates/zeta/src/database.rs`, applied at startup. Never edit committed migrations; append new ones. Versions sort numerically and history is mixed; current convention is `date -u +%y%m%d%H%M%S` (e.g. `261005000000_…`). Don't use `date +%s` — it sorts before every existing migration.
- `#[sqlx::test]` needs `DATABASE_URL` naming an **existing** database (cloned per test); without postgres: `Connection refused (os error 111)`. Don't rewrite DB code over connection errors — check `docker compose ps` or use `--lib`. Local: `docker compose up postgres` + `export DATABASE_URL=postgresql://user:changeme@localhost/zeta_development`.
- Repository error enums: `impl_repository_error!(Error, "entity")`. Repository methods get their DB span from `#[instrument_database(operation = "SELECT", collection = "…")]` (needs a `db` field on the struct).

## Shared abstractions (reuse, don't reimplement)

- `zeta-http`: `get_json(_or_404)` is the standard JSON call; `parse_response` + `ApiError`; `emulated::builder` (wreq browser emulation) for anti-bot sites — plain `reqwest` gets TLS-fingerprinted; `oauth::TokenCache` for client-credentials APIs.
- Transport errors: only `zeta-http/src/error.rs` and the client crates' own error wrappers may hold raw transport errors or call `without_url()`/`without_uri()` — enforced by the `no_raw_transport_errors_outside_the_wrapper` test. Plugin errors use `Api(#[from] http::ApiError)`; client build errors: `.map_err(|error| Error::Request(error.into()))?`.
- `zeta-url`: `text.urls()`, path-segment validators (segments stay percent-encoded), `RedactedUrl` — required for every logged URL and transport error.
- `zeta-cache` `TtlCache`/`TtlMap`: the refresh lock is held across `refresh().await`; a refresh closure must never call back into the same cache (deadlock).
- `zeta-utils`: `labeled_reply`/`plain_reply` (re-exported via `zeta_plugin::irc` with `COLOR`/`BOLD`/`RESET` — replies are hand-composed IRC control codes, no formatting crate), `append_entries_within_budget`, `Truncatable`, `resolve_setting` (config → env → default), `duration`, `sync::{read,write,lock}` for poison recovery.
- Secrets: `zeta_plugin::resolve_secret(settings.x.as_deref(), "ENV_VAR")`. Mirroring: `ensure_mirrored_if_configured` (`plugins-shared/src/mirror.rs`).

## Plugins and events

- Adding a plugin:
  1. Create `crates/plugins-<name>/` with the plugin in `src/lib.rs`; annotate its inherent impl with `#[plugin(name = "<name>", settings = …)]` (examples: `plugins-geoip` for `#[command]`, `plugins-tiktok` for `#[handler(url)]`).
  2. In `crates/plugins/Cargo.toml` add: the optional dependency, the `dep:plugins-<name>` feature, and entries in `all`, `native-tls`, `native-tls-vendored`, `rustls` (Cargo can't derive these).
  3. In `crates/plugins/src/lib.rs` add `pub use plugins_<snake> as <snake>;` and the name to `BUNDLED_PLUGIN_NAMES`. Tests `every_plugin_crate_is_wired_into_the_facade` / `every_bundled_plugin_name_has_a_registration` catch misses.
- `#[command]` generates the `CommandSpec`, registration and parse-or-usage dispatch (args parse with `ArgsMode::Shell`; `words` flag → `ArgsMode::Words`; `label = "…"` for usage replies). Commands are always-on; gate with `register_if = <expr>` (`settings` in scope) or use `#[handler(command)]` + `zeta_plugin::irc::parse_or_usage(client, command, ArgsMode::…, fmt)` for dynamic dispatch. `#[handler(kind)]` forwards any trait handler verbatim. Constructors are sync — spawn background work in `loaded`.
- Naming: crate `plugins-<kebab>`, plugin name and `[plugins.<name>]` section snake_case, cargo feature kebab-case (`plugins-rust-playground` / `rust_playground` / `rust-playground`).
- Larger plugins grow `src/<name>/` (URL matchers in `urls.rs`); DB-backed ones use the repository/service/model split — see `plugins-alert/`.
- Handlers: triggers are single words; `UrlScope::Hosts` matches the exact lowercased host (list `imdb.com` **and** `www.imdb.com`); `titles` is the `UrlScope::Any` fallback. Handler calls are serialized per plugin — spawn a task for long work.
- New event kind: update the `event_kind_flags!` table (`zeta-plugin/src/event.rs`), the hand-written `Plugin::handle_*` default (`zeta-plugin/src/plugin.rs`), and the routing in `zeta-plugin-runtime/src/dispatch.rs`. The defaults can't be macro-generated (`#[async_trait]` → E0195); don't retry.

## zeta-unicode

- Read `crates/zeta-unicode/README.md` first (flagged as loosely verified AI-generated code). `build.rs` trains the Nomina image from UCD files in `crates/zeta-unicode/data/` at compile time; nothing generated is committed. Format changes touch `zeta-unicode-build/src/nomina/{names,emoji,gc,patterns,image}.rs`, the runtime's `zeta-unicode/src/nomina/{names,emoji,gc,image}.rs` (patterns decode in `image.rs`), and `zeta-unicode-format` together.
- Updating Unicode: `hack/fetch-ucd.sh <version> <scratch-dir>`, then copy the five files into `data/` (mismatched headers fail). `UNICODE_VERSION` is generated; don't edit it. Verify with `ZETA_UNICODE_DATA_DIR=... cargo test -p zeta-unicode --all-features` or `cargo run -p zeta-unicode-build --bin zeta-unicode-train -- --data-dir ... --out-dir ... --report --verify`. `ZETA_UNICODE_STATS=1 cargo build -p zeta-unicode` prints the candidate table; `ZETA_UNICODE_PRIORITY=latency|balanced|size` picks the block size.
- Image budget is 200,000 B (actual 181–199 KB); the old ~100 KB full-decode designs are not the production path. Pre-Unicode-10 data is unsupported (`MissingDerivedName`).

## Running locally

- `docker compose up` → postgres :5432, ircd :6667, Jaeger :16686; `--profile mirror` adds MinIO :9000 (`zeta`/`changeme`, bucket `zeta-mirror`). `docker compose restart ircd` after editing `hack/inspircd/testing.conf`.
- `cargo run` reads `./config.toml` (`-c/--config-path` to override); for the local ircd set `hostname = "localhost"` and `[irc.tls] enabled = false`.
- Mirror E2E needs `ZETA_YTDLP_COMMAND` and `ffmpeg`.
- Scripted IRC clients must answer the handshake `PING :<cookie>` immediately, or nothing after it works.

## Telemetry

- Only **spans** are exported (OTLP) plus JSON stdout; events outside a span are dropped. Log inside `irc.dispatch`, `zeta.event.handle`, `http.client` or an `#[instrument]`ed fn; background tasks open their own per-unit span.
- One span per unit of work; no process/task-lifetime spans (`run`, `*_scheduler`, `*_actor`). Async fan-out (`titles.fetch`, `unwall.resolve`, `media.download`) is created as a child of the handler span while it is current.
- Span names: low-cardinality `{domain}.{operation}` (`imdb.search`, `alert.deliver`); HTTP client spans are `http.client`; DB spans `{operation} {target}`. Repeated spans under one parent put a bounded discriminator in `otel.name` (`zeta.event.handle {plugin} {kind}`). Never a URL, id, channel or nick in a name.
- Spans that parent client spans set `target: "zeta.plugins"` so a `RUST_LOG` filtering plugin crates can't orphan the HTTP/DNS/S3 children.
- Attribute names follow semconv: `url.full` (via `RedactedUrl`), `http.request.method`, `http.response.status_code`, `server.address`, `error.type`, `exception.message`, `db.*`, `dns.*`, `aws.s3.key`, `file.path`, `irc.channel`, `zeta.plugin.name`, `zeta.event.kind`, `media.id`, `search.query`. No bare `plugin`, `id`, `count`, `query`, `name`, `path`, `key`, `body`, `channel`, `error`, `err`, `e`.
- Levels: `error` only for unhandled/data-loss/fatal; `warn` for handled-but-unexpected; `debug` for per-operation detail and expected misses (404s, user errors); `trace` for per-line payloads. One failure, one log line, at the layer with the unique context. Level sets span status.
- Default filter (`crates/zeta/src/telemetry.rs`): `zeta=info,plugins=info` plus `debug` for the client crates; set `RUST_LOG=…,plugins=debug` / `zeta=debug` for per-operation detail.

## Configuration

- `config.toml` + `ZETA_`-prefixed env (`__` separates sections, e.g. `ZETA_IRC__NICK_PASSWORD`); arrays can't come from env. `repository_config_parses` fails `cargo test` if the repo `config.toml` breaks.
- `[plugins] enabled_by_default` (default `true`) decides what an omitted `enabled` means.
- Unknown sections/keys are warned, never rejected — don't add `deny_unknown_fields` (only `[mirror]` uses it).
- `.envrc` (gitignored) holds 1Password `op://` URIs; a literal `op://…` value lets a plugin load but fail every request. Resolve with `op read`.

## Deliberate decisions (don't undo)

- Keep the crate split; don't fold crates together.
- Per-plugin API error enums stay hand-written; the client crates' duplicated error wrappers stay duplicated.
- No `Plugin::metadata()` — names come only from `PluginName::NAME`.
- `wreq`/BoringSSL stays in default features (dropping it silently disables titles/instagram).
- The presence/CTCP/raw event API has no users yet — intended.
- API response and DB row models keep **all** fields (`#[allow(dead_code)]` where unread).
- `TokenCache::get` holds its write guard across the request on purpose (single-flight).
- `PluginRuntime::shutdown` drops the `EventIndex` before draining tasks so plugins get `shutdown`; stragglers are aborted at the grace deadline.
- `ofn`/`unwall` hardcode the `irc.rwx.im:6697` network id (known TODO).
- `crates/plugins-gay/src/lib.rs` is an intentional joke module — don't refactor it.

## Style & process

- Docs: `# Errors`/`# Panics` sections, `#[must_use]` on pure constructors, `Default` delegating to `new()`. Brand names in prose, backticks only for code.
- Fix lints by hand; never run `cargo clippy --fix` / `cargo fix` across the workspace (pedantic/nursery autofixes are destructive).
- Rustdoc: in a module whose `mod` line carries an outer `///` doc (e.g. modules in `crates/zeta/src/lib.rs`), its `//!` links need full `crate::` paths.
- Releases: tag `0.x.y` must equal `version` in `crates/zeta/Cargo.toml` (checked by `release.yaml`).
- Commits: short imperative subjects. PRs target `main`, title under 50 chars, unwrapped body. Only include a verification section when it's relevant beyond the basic cargo commands.
- `AGENTS.md` is tracked in the repository: keep it up to date when conventions change.
