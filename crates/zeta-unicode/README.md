# zeta-unicode

Unicode character names, general categories and emoji sequence names, trained at build time
into one flat Nomina image and linked into the binary with `include_bytes!`. Nomina is Latin
for names.

The image is generated from the vendored UCD files by `zeta-unicode-build`. There is no
checked-in derivation-rules file and no runtime compression library:

- character names are front-coded against their code-point predecessor and coded with
  canonical order-1 Huffman models; the global layout checkpoints each block in the stream so
  a lookup decodes one checkpoint and at most one block;
- Hangul syllables and `*`-pattern names are generated from the code point;
- emoji names are generated from the lowercase UCD name where a verified grammar rule
  reproduces the published CLDR name, with literal fallback;
- general-category runs are delta-varint and Huffman streams, rebuilt into the existing
  two-level page table at startup;
- emoji keys, names and tone rules are compact Huffman/bit-packed streams.

The runtime keeps one name decoder per thread (fixed 256-byte scratch, zero allocations per
lookup) and one process-wide emoji and general-category table.

## Layout

| Section | Content |
|---|---|
| 0 | character names: page directory, runs, block offsets/checkpoints, Huffman models and stream |
| 1 | emoji: canonical keys, literal names, grammar rules, packed tone codes and masks |
| 2 | general category: run-start and category-id streams |
| 3 | `*`-pattern ranges and prefix/suffix dictionaries |

Measured with Unicode 18.0.0 and the default balanced profile:

| | bytes |
|---|---:|
| names | 174,091 |
| emoji | 10,548 |
| general category | 3,860 |
| patterns | 306 |
| **image (with header/padding)** | **188,882** |

The image budget is 200,000 bytes. `ZETA_UNICODE_PRIORITY=latency` selects 32-name blocks
(199,258 bytes, ~1.2 µs random name lookup); `size` selects 256-name blocks (181,026 bytes,
~7.7 µs); `balanced` is the default at 64-name blocks (~2.3 µs).

## Training from a UCD release

Fetch a release with `hack/fetch-ucd.sh <unicode-version> <dest> [emoji-version]`, or point
the trainer at an existing UCD directory. It accepts both a flat vendored directory and an
extracted release root:

```sh
cargo run -p zeta-unicode-build --bin zeta-unicode-train -- \
    --data-dir ../ucd/15.1.0 \
    --out-dir target/nomina-15.1 \
    --priority balanced \
    --report --verify
```

It writes `nomina.bin`, `manifest.json` (versions, relative input paths, sizes, FNV-1a
hashes, section sizes, candidate table, chosen profile and verification status) and prints the
candidate table. Inputs may be laid out as either

```text
<data>/{DerivedName,DerivedGeneralCategory,NameAliases,NamedSequences,emoji-test}.txt
<data>/ucd/extracted/DerivedName.txt
<data>/ucd/extracted/DerivedGeneralCategory.txt
<data>/ucd/NameAliases.txt
<data>/ucd/NamedSequences.txt
<data>/emoji/emoji-test.txt
```

`zeta-unicode` itself builds through its build script, which uses the same trainer:

```sh
ZETA_UNICODE_DATA_DIR=/path/to/ucd \
ZETA_UNICODE_PRIORITY=balanced \
ZETA_UNICODE_MAX_BYTES=200000 \
cargo build -p zeta-unicode
```

`ZETA_UNICODE_BLOCK_SHIFT` pins a block size (5..8) instead of searching.
`ZETA_UNICODE_STATS=1` prints the candidate and section report.

## Supported Unicode versions

`DerivedName.txt` exists from Unicode 10.0 onward; older releases fail with a clear
`MissingDerivedName` error rather than silently generating incomplete names. The version
headers of all inputs are validated against each other, and `emoji-test.txt` may use the older
comment shape (`# 😀 name`) or the modern one (`# 😀 E5.0 name`).

The runtime tests read the same `ZETA_UNICODE_DATA_DIR` when it is set, so the full name,
emoji, general-category and pattern verification can be run against an older release:

```sh
ZETA_UNICODE_DATA_DIR=/path/to/ucd-15.1 cargo test -p zeta-unicode --all-features
```

## Verification

Every training run replays all explicit names, Hangul syllables and pattern code points,
every stored emoji entry and all 1,114,112 general-category code points before writing the
image. The build also pins the image budget and `UNICODE_VERSION` against the image header.
