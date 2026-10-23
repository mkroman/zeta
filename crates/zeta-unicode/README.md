# zeta-unicode

> [!WARNING]
> **AI slop — loosely verified.** This crate (the encoder, build script, data handling and this
> README) was written almost entirely by an AI and reviewed only lightly by a human. The
> generated table *is* checked at build time against the vendored UCD files, and the crate's
> tests round-trip every name, but treat the rest — the design notes, the performance claims and
> the size figures below — as unverified marketing until you have measured them yourself. Read
> the code before you trust it.

Unicode 18.0.0 character names, general categories and emoji sequence names for the zeta IRC
bot, compiled from the vendored UCD files in `data/` into one packed table by
`zeta-unicode-build`.

```rust
assert_eq!(zeta_unicode::name('ß').unwrap(), "LATIN SMALL LETTER SHARP S");
assert_eq!(zeta_unicode::general_category('ß').long_name(), "Lowercase Letter");
assert_eq!(zeta_unicode::emoji_name("🏳️‍🌈").unwrap(), "rainbow flag");
assert_eq!(zeta_unicode::name_or_label('\u{f8ff}'), "<private-use-F8FF>");
```

## The contract

| requirement | value | why |
|---|---|---|
| generated table | **≤ 100,000 bytes** | the hard budget this crate exists to meet |
| lookup cost | O(1) table walk or a short bounded decode; **no per-lookup decompression** | `.uni` answers inside an IRC handler; a reply must not pay a codec |
| one-time init | allowed, measured in ms and heap bytes | a lazily decoded section must not hide per-query cost |
| Rust | safe only (`unsafe_code = forbid`), bounds-checked reads | workspace rule |
| targets | **x86_64 and aarch64**, same structure for both | the bot ships for both; no ISA-specific tricks |

### What is measured today

The names are stored as three PPMd8-I streams and decoded once; everything else is still stored
raw, so the table is over budget on purpose until the last two pieces land. The ledger
(`table_stays_within_the_encoder_limit`, or `ZETA_UNICODE_STATS=1` at build time):

| section | bytes | note |
|---|---:|---|
| names | 69,062 | `UNNM` container: three PPMd8-I streams, decoded once at first lookup |
| name runs | 1,066 | `varint(len), varint(gap)` per run, 719 runs, code point → name id |
| name patterns | 691 | the 20 `*` ranges |
| **names + index** | **70,819** | **budget 73,304 — met**, or 72,717 charging the shared literals to the table |
| general category | 9,494 | two-level page table: 545 `u16` directory entries over 4,202 packed `u16` records |
| emoji | 45,659 | 1,944 stored keys answer for all 3,972 emoji sequences (see below) |
| header + directory | 72 | magic, version, section count and five 12-byte directory entries |
| **total** | **126,044** | budget 100,000 — the last two sections are still stored raw |

**Names.** Each name is front-coded against its code point predecessor — shared prefix length,
shared suffix length, NUL-terminated middle — and the three streams are coded separately, because
their statistics differ (prefix lengths are delta-8 coded, the middles are raw). The first lookup
decodes them once into a flat `Vec<u8>` plus a `Vec<u32>` of offsets; afterwards a lookup is a
binary search over the runs and two `u32` loads.

**Names as code.** 6,677 of the 35,416 names are not stored at all: `data/derived-names.rules`
holds 5,286 rules over 324 shared literals, and `zeta-unicode-build` turns them into a balanced
comparison tree in generated Rust (`$OUT_DIR/zeta_unicode_derive.rs`) that rebuilds each name
from its code point. Those names keep their id but take no space in the streams, and because the
decoder walks ids in ascending order they remain valid front-code predecessors. The tree runs
once per name at startup and never on a lookup, so its size cannot affect lookup latency; the
generator verifies every rule against `DerivedName.txt` and the crate's round-trip test checks
them from the decoder side. This is where the names budget was won: the same streams without the
rules are 78,910 B.

Two ideas carry the emoji section. **Keys are canonicalized**: `U+FE0F` is stripped, so
text-presentation input answers like emoji presentation (`🚶‍♂` → `man walking`) at no storage
cost, and the keycap named sequences take precedence over the test file. **Skin-tone
variants are never stored**: each of the 344 tone groups gets a 3-bit rule — insert the tone
phrase after the first `": "` or append it, collapsing duplicate tones or not — plus a 2-bit
mask of the tone counts the group publishes, and lookup strips the tones from the query, checks
the published count, finds the base and renders the name. Twelve groups whose tone-stripped
sequence is not itself an entry get their base reconstructed (by inverting the rule on one
variant and checking it against the whole group), so **zero** tone names are stored verbatim.

Measured on the current port (release, x86_64 — rerun the harness before trusting the numbers
elsewhere): `general_category` 6.1 ns, `name` 19.2 ns, `emoji_name` 224 ns. The `name` figure is
a binary search over the 719 runs plus two offset loads and a slice of text that was validated
once while decoding; the first name lookup also materializes the ~0.9 MB name blob plus the
codec's transient model (16 MiB for the middle stream), both released before the lookup returns.
`emoji_name` is dominated by the tone fallback that has to render a name the table does not
hold.

### What is left to fit the budget

The names are done and inside their budget. The last step is the general category and emoji
sections, which are still stored raw: 9,494 → 7,704 for the general category, and the emoji
section (45,659 B) under lzma-9e. The design study measured 60,890 → 21,400 for the larger
pre-filter section, so the filtered one should compress below that; front-coding the emoji names
the way the character names are should take it further down. Together with the name runs and
patterns (1,757 B) that lands the whole table under 100,000 B.

### How bytes are spent: code first, data second

The build script may emit **Rust code**, not only bytes, and that is not a loophole — it is the
cheapest place for anything derivable:

* **Derivation rules are code.** Hangul syllables, `*` patterns (`CJK UNIFIED IDEOGRAPH-4E00`),
  and family rules (`chr(c0 + step·(cp − start))`, hex suffixes, number words) cost *zero* table
  bytes when they are functions, and the instructions are shared by every code point they cover.
  A rule stored as a record costs 4–8 bytes per occurrence; as code it costs a few dozen bytes
  once.
* **Fragments live in `&'static str` literals.** The linker merges identical constants, so a
  template text shared by 300 names is paid once, in `.rodata`, not once per name.
* **Indices stay data.** A two-level table beats a generated `match` on ranges: 719 `match` arms
  compile to a compare tree of roughly 10–20 bytes each of `.text` (and i-cache pressure) versus
  4,320 B of data with a predictable 10-probe walk. Tables where a load and a mask beat a branch
  are data; content that only makes sense as a procedure is code.
* **Corpora are compressed.** Prose (names, CLDR names) is what codecs are for; it gets an
  encoding chosen by measurement, decoded at most once.

### x86_64 and aarch64

Both are 64-bit with cheap loads, so the layout rules are about keeping every lookup to a couple
of predictable memory touches:

* fixed-width, **naturally aligned** records (`u16`/`u32`), never varint walks — a varint scan is
  a loop of dependent byte loads and branches, and it decodes differently on both ISAs;
* records do not straddle cache lines when that is free to avoid (6-byte records are acceptable
  for a 4 KB index that stays in L1, measured 16 ns);
* shallow branch depth: ≤ 2 branches before a probe loop, ≤ ~12 probes per lookup (measured:
  general category 2.34 probes average, 12 max, 3.70 ns);
* no SIMD, no `std::arch`, no assumptions about unaligned-load cost — aarch64 pays more for a
  load that crosses a line than x86_64 does;
* the crate ships a benchmark harness; run it on both architectures before changing a layout.

## Updating the Unicode data

Replace the files in `data/` from `https://www.unicode.org/Public/<version>/`, then:

```sh
cargo test -p zeta-unicode            # round-trips every name against the new files
ZETA_UNICODE_STATS=1 cargo build -p zeta-unicode   # prints the table and section sizes
```

`data/derived-names.rules` is the exception: its rules are mined from the UCD, not derived by
formula, so they ship with the data they were mined from. `zeta-unicode-build` checks every rule
against `DerivedName.txt` while it builds, and the round-trip test covers them from the other
side.

`zeta-unicode-build` parses the `# <File>-<version>.txt` headers of all five files and refuses to
build if they disagree, so a partial update cannot ship. The stats line also reports how the
emoji section decomposed — stored entries, ruled tone groups, verbatim variants and groups with
no base — which is the signal to watch when a new Unicode release adds sequences.

## Data and license

Tables are derived from the Unicode Character Database (© 1991–2026 Unicode, Inc.), Unicode
License v3 — see `LICENSE-UNICODE`. The code is dual-licensed under MIT or Apache-2.0.
