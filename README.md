# Zeta

Zeta is an opinionated IRC bot with a bunch of plugins.

It is also an excercise in futility.

## Workspace layout

The bot is split into focused crates with a strict dependency direction: the host depends on
the plugins, the plugins depend on the plugin interface, and the shared infrastructure sits in
leaf crates below both.

| Crate | Purpose |
| --- | --- |
| `zeta` | The IRC bot: client loop, configuration, tracing, migrations. |
| `plugins` | The bundled plugins facade: one feature per plugin, plus the plugin-name list and feature graph. |
| `zeta-plugin` | The plugin contract: trait, events, commands, context, reply conventions. |
| `zeta-plugin-runtime` | Plugin registry, event dispatch and grace-bounded shutdown. |
| `plugins-shared` | The few helpers used by more than one bundled plugin (URL matchers, mirror replies). |
| `zeta-http` | HTTP wrappers, browser emulation, OAuth2, transport-error redaction. |
| `zeta-mirror` | Media mirroring: `yt-dlp` downloads uploaded to S3. |
| `zeta-cache` | TTL cache and keyed cache APIs. |
| `zeta-url` | URL extraction, path validation and redaction. |
| `zeta-utils` | Text shaping, duration formatting, lock recovery. |
| `zeta-unicode` | Nomina: character names, general categories and emoji sequence names. |
| `zeta-unicode-build` | Offline trainer and generator for the `zeta-unicode` image. |
| `zeta-unicode-format` | Nomina wire layout, byte primitives, Huffman codec and emoji grammar shared by the two Unicode crates. |
| `kagi`, `reddit`, `dendanskeordbog`, `coinmarketcap` | Standalone API client crates. |
| `zeta-test-support` | Shared test helpers (wiremock, redaction assertions). |

### Unicode stack

`zeta-unicode` does not ship a checked-in table. `zeta-unicode-build` trains one flat Nomina
image from the UCD files vendored under `crates/zeta-unicode/data`; the image holds character
names, general categories, emoji sequence names and the algorithmic `*` pattern rules.
The `zeta-unicode` build script runs the same trainer at compile time and links the verified
image into the binary, while `zeta-unicode-format` holds the wire constants, byte primitives,
Huffman codec and emoji grammar shared by the trainer and the runtime. Character names decode
per thread; emoji and general-category tables are decoded once per process. The `string-utils`
plugin uses the runtime for `.uni`.

The three Unicode crates are flagged as loosely verified AI-generated code; read the warning in
[`crates/zeta-unicode/README.md`](crates/zeta-unicode/README.md) before trusting the design
notes. The full format and data-flow overview is there too, with the generator and shared
format documented in [`crates/zeta-unicode-build/README.md`](crates/zeta-unicode-build/README.md)
and [`crates/zeta-unicode-format/README.md`](crates/zeta-unicode-format/README.md).

Two rules keep the graph honest:

- **API client crates** exist for domain APIs that stand alone (or are consumed by more than
  one plugin): they carry their own model, error and transport-redaction wrapper, and never
  depend on the plugin interface. Plugins with a single consumer keep their client in the
  plugin (e.g. `imdb`, `unwall`).
- **`zeta-plugin` is only the contract** between the host and the plugins — anything that is
  implementation rather than vocabulary belongs in an infrastructure crate.

## License

Licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)
 
at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
