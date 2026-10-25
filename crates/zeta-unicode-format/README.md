# zeta-unicode-format

> [!WARNING]
> **AI slop — loosely verified.** This crate (the encoder, build script, data handling and this
> README) was written almost entirely by an AI and reviewed only lightly by a human. The
> generated table *is* checked at build time against the vendored UCD files, and the crate's
> tests round-trip every name, but treat the rest — the design notes, the performance claims and
> the size figures below — as unverified marketing until you have measured them yourself. Read
> the code before you trust it.

## What it does

Shared Nomina layout, byte primitives, canonical Huffman codec and emoji grammar for
[`zeta-unicode`](../zeta-unicode/README.md) and
[`zeta-unicode-build`](../zeta-unicode-build/README.md). It has no dependencies: the trainer
uses it to encode and the runtime to decode, so both sides share one definition.

## Wire contract

The 64-byte header is `NOM1`, three `u16` version fields, section count, name-block shift, GC
page shift, name layout and four `(offset u32, length u32)` entries, padded to 64 bytes.
Sections are names (0), emoji (1), general category (2) and patterns (3). `NameLayout` ids are
`0` block-local and `1` global checkpointed; `GC_SHORT_NAMES` fixes the 30 category ids.

Wire ids are stable. Changing a layout id, rule id, section id or category order reinterprets
existing images, so update both crates and regenerate the image.

## Huffman codec

Streams are canonical Huffman over bytes: `u8 context_count`, then per context `u8 context`,
`u8 symbol_count` and `(u8 symbol, u8 length)` pairs, then an MSB-first bit payload. Order 0
uses one context; order 1 selects by the previous byte. Codes are length-limited by halving
frequencies when the tree is too deep. `valid_codebook` checks the Kraft inequality; decoding
rejects truncated codebooks, invalid codes and short payloads. The runtime adds a 6-bit fast
table per name model.

## Limits

Both stream counts are `u8`, so a stream carries at most 255 contexts and at most 255 of the
256 byte symbols in one context; a corpus that needs all 256 contexts or all 256 symbols in one
context requires a format change. Codes are at most 32 bits. `decode` rejects a `raw_len` larger
than eight times the stream length, because every symbol consumes at least one bit. `encode`
and the codebook helpers panic outside these limits rather than emitting a stream the decoder
cannot trust.

## Emoji grammar

Most names are rules, not stored strings. The trainer keeps a rule only when replay reproduces
the published CLDR name exactly; the runtime replays it. Rules are literal, `ucd_mask`,
`ucd_mask_hyphen`, keycap, gender swap (woman/man), facing, family, couple, kiss, pair, tone
component, clock and `keycap: 10` (ids 0–13).

Tone code points and names, `format_toned`/`tone_phrase`, family/keycap/pair helpers,
`ucd_mask_name` and `Rule::pack`/`unpack` live here. A wording change still needs a new image
and verification run.

## Testing

```sh
cargo test -p zeta-unicode-format --all-features
```

Tests cover varints, hex padding, UTF-8, packed fields, both Huffman orders, truncation, Kraft
validation, rule ids and grammar helpers.
