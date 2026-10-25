# zeta-unicode

> [!WARNING]
> **AI slop — loosely verified.** This crate (the encoder, build script, data handling and this
> README) was written almost entirely by an AI and reviewed only lightly by a human. The
> generated table *is* checked at build time against the vendored UCD files, and the crate's
> tests round-trip every name, but treat the rest — the design notes, the performance claims and
> the size figures below — as unverified marketing until you have measured them yourself. Read
> the code before you trust it.

## What it does

Runtime side of Nomina: character names, general categories and emoji names in one image
generated at build time and linked with `include_bytes!`. There is no checked-in table and no
runtime compression library.

[`zeta-unicode-build`](../zeta-unicode-build/README.md) is the offline trainer;
[`zeta-unicode-format`](../zeta-unicode-format/README.md) is the shared wire contract and
codec. The trainer is only a build-dependency.

## Architecture

```text
UCD files ──► zeta-unicode-build ──► image + manifest
                    (offline)              │
                                           ▼
             zeta-unicode build script ──► linked runtime ──► `.uni`
```

The trainer reads five UCD files, parses names plus aliases, encodes the four sections and
tests several name-block sizes against a size budget. It verifies every section, picks the
best layout, and writes the image plus a deterministic manifest. The build script runs the same
pipeline and links the image and `UNICODE_VERSION` into the crate.

## Runtime

The name decoder and pattern table are per-thread, so name lookups need no lock. Emoji and
general-category tables are process-wide `OnceLock`s warmed by `init()`. A malformed image
panics on first use; the build verifies it, so that is an invariant.

- **Name:** code point → page → run → name id → one front-coded block. Stored names use two
  fixed 256-byte buffers; Hangul and pattern names are generated.
- **General category:** decode the run streams once, then binary-search one 2,048-code-point
  page of a two-level table.
- **Emoji:** strip `U+FE0F`, binary-search the key arena, then materialize a rule. Unknown keys
  are retried with skin tones stripped to synthesize the base variant.

`write_name` allocates nothing; `name` allocates only its result. The public API is `name`,
`write_name`, `name_or_label`, `general_category`, `is_noncharacter`, `emoji_name`,
`may_start_emoji_sequence`, `init`, `UNICODE_VERSION`, `TABLE_BUDGET` and
`MAX_EMOJI_KEY_BYTES`.

## Nomina format

A 64-byte header holds `NOM1`, the Unicode version, section count, name-block shift, GC page
shift, name layout and four `(offset, length)` entries; four 8-byte-aligned sections follow.

| Section | Encoding | Runtime form |
|---|---|---|
| names | front-coded blocks with canonical Huffman models | per-thread decoder |
| emoji | four Huffman streams plus packed tone rules | process-wide arenas |
| general category | two Huffman streams of run starts and ids | process-wide page table |
| patterns | prefix/suffix dictionaries plus range records | per-thread range list |

- **Names:** 35,416 names in 554 blocks. The global layout front-codes against the previous
  code point, shares prefix and suffix, and checkpoints each block after the first. The
  block-local layout resets per block; the runtime accepts both, training emits global.
- **Emoji:** 1,944 base entries (1,341 rule-generated, 603 literal) cover 3,972 canonical
  sequences. Most names are replayed from 14 grammar rules instead of stored; tone variants are
  synthesized from the base.
- **General category:** 4,202 runs rebuild a 545-page table; lookups binary-search one page and
  use 9,494 bytes of heap.
- **Patterns:** 20 `*` ranges use shared prefix/suffix dictionaries and generated hex names.

Unicode 18.0.0 with the default profile is 188,882 bytes: 174,091 B names, 10,548 B emoji,
3,860 B general category, 306 B patterns and 77 B header/padding. The budget is a hard
200,000-byte cap shared by the trainer and the runtime; the build script will not embed an
image over it.

| Block shift | Image | Preferred by |
|---:|---:|---|
| 5 (32 names) | 199,258 | `latency` |
| 6 (64 names) | 188,882 | `balanced` (default) |
| 7 (128 names) | 183,658 | |
| 8 (256 names) | 181,026 | `size` |

## Building and training

Inputs are `DerivedName.txt` (Unicode 10+), `DerivedGeneralCategory.txt`, `NameAliases.txt`,
`NamedSequences.txt` and `emoji-test.txt`. `NamedSequences.txt` is optional before Unicode 4.1;
the Emoji version is independent. Build settings are `ZETA_UNICODE_DATA_DIR`,
`ZETA_UNICODE_PRIORITY`, `ZETA_UNICODE_MAX_BYTES`, `ZETA_UNICODE_BLOCK_SHIFT` and
`ZETA_UNICODE_STATS`. `ZETA_UNICODE_MAX_BYTES` can only lower the fixed 200,000-byte budget;
the build fails on a larger value rather than embedding an over-budget image.

```sh
ZETA_UNICODE_DATA_DIR=/path/to/ucd cargo build -p zeta-unicode

cargo run -p zeta-unicode-build --bin zeta-unicode-train -- \
    --data-dir ../ucd/15.1.0 --out-dir target/nomina --report --verify
```

`hack/fetch-ucd.sh <unicode-version> <dest> [emoji-version]` fetches a release into the
scratch directory `<dest>`. It refuses a `<dest>` that already holds any earlier candidate
layout the trainer probes first (flat, `extracted/` or `ucd/`), because those shadow the
fetched file; update the vendored `crates/zeta-unicode/data` by copying the five files into it
instead. The CLI writes `nomina.bin` and `manifest.json`; `--verify` re-reads the image. The
manifest records versions, profile, sections, candidates, input hashes and verification checks,
with no timestamps or absolute paths. Training is deterministic.

## Verification and testing

The trainer re-decodes every section and re-parses the image before accepting it. The manifest
checks are `name-stream-round-trip`, `emoji-stream-round-trip`, `general-category-round-trip`,
`pattern-round-trip` and `image-layout`. Bad UCD records and verification mismatches panic;
input, profile, budget, baseline and I/O failures are errors.

Runtime tests re-read the UCD and round-trip names, aliases, categories and the emoji oracle;
they also check `UNICODE_VERSION` and `TABLE_BUDGET`.

```sh
cargo test -p zeta-unicode --all-features
cargo test -p zeta-unicode-build --all-features
cargo test -p zeta-unicode-format --all-features
cargo test -p plugins --all-features string_utils
cargo test -p zeta-unicode --release -- --ignored --nocapture benchmarks
```

## `.uni`

`string-utils` warms the shared tables, names characters, reads categories, groups emoji
sequences and bounds probing with `MAX_EMOJI_KEY_BYTES`. The reply budgets both the listing and
each sequence's code-point list.

See [`zeta-unicode-build`](../zeta-unicode-build/README.md) and
[`zeta-unicode-format`](../zeta-unicode-format/README.md) for the other halves.
