//! Benchmarks for URL extraction.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use zeta::url::{ExtractUrls, ExtractUrlsExt, SchemeMap};

/// A message without any URLs — the common case that must be rejected quickly.
const PLAIN_MESSAGE: &str = "lol yeah i saw that too, pretty wild stuff honestly, anyway are you around later today or what";

/// A typical chat message: mostly plain words with a couple of URLs.
const CHAT_MESSAGE: &str = "haha yeah that was wild, check this out https://maero.dk/posts/2026/rust-irc-bots someone posted it earlier and also https://example.com/some/path?query=1#fragment if you missed it";

/// A message with broken schemes that need repair.
const TTP_MESSAGE: &str = "fixed the link for you ttps://maero.dk/posts/2026/rust-irc-bots and also ttp://example.com/a/path in case the other one died again lol";

/// Scheme map with the broken `ttp`/`ttps` variants, as used by the titles plugin.
const TTP_SCHEMES: SchemeMap = &[
    ("http", None),
    ("https", None),
    ("ttp", Some("http")),
    ("ttps", Some("https")),
];

/// Counts the URLs extracted from `message` with the default schemes.
fn extract(message: &str) -> usize {
    black_box(message).urls().count()
}

/// Counts the URLs extracted from `message` with the given schemes.
fn extract_with_schemes(message: &str, schemes: SchemeMap) -> usize {
    ExtractUrls::with_schemes(black_box(message), schemes).count()
}

fn bench_extraction(c: &mut Criterion) {
    let messages = [
        ("plain message", PLAIN_MESSAGE),
        ("chat message", CHAT_MESSAGE),
        ("ttp message (default schemes)", TTP_MESSAGE),
    ];

    let mut group = c.benchmark_group("url extraction");

    for (name, message) in messages {
        group.throughput(Throughput::Bytes(message.len() as u64));
        group.bench_function(name, |b| b.iter(|| extract(message)));
    }

    group.throughput(Throughput::Bytes(TTP_MESSAGE.len() as u64));
    group.bench_function("ttp message (ttp schemes)", |b| {
        b.iter(|| extract_with_schemes(TTP_MESSAGE, TTP_SCHEMES));
    });

    group.finish();
}

criterion_group!(benches, bench_extraction);
criterion_main!(benches);
