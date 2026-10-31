//! Heap-allocation regression test for [`path_segments`].
//!
//! The allocator counters are process-wide, so both cases live in a single test: tests in one
//! binary run in parallel, and a second test allocating between the snapshots would inflate the
//! counts.

use std::alloc::System;

use stats_alloc::{INSTRUMENTED_SYSTEM, Region, Stats, StatsAlloc};
use url::Url;

use zeta_url::path_segments;

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

/// Returns the allocations and allocated bytes `f` performs.
fn measure(f: impl FnOnce()) -> Stats {
    let mut region = Region::new(GLOBAL);

    f();

    region.change_and_reset()
}

#[test]
fn path_segments_keeps_typical_urls_off_the_heap() {
    let typical = Url::parse("https://www.example.com/a/b").expect("valid URL");

    let typical_stats = measure(|| {
        let segments = path_segments(&typical).expect("the URL has segments");

        assert_eq!(segments.as_slice(), ["a", "b"]);
    });

    // On main this collected into a `Vec<&str>`: one allocation of 64 bytes (a two-element
    // vector reserves four 16-byte slots).
    assert_eq!(typical_stats.allocations, 0, "the segments live inline");
    assert_eq!(typical_stats.bytes_allocated, 0, "nothing reaches the heap");

    // Twelve segments overflow the inline buffer, which spills to the heap exactly once.
    let deep = Url::parse("https://www.example.com/a/b/c/d/e/f/g/h/i/j/k/l").expect("valid URL");

    let deep_stats = measure(|| {
        let segments = path_segments(&deep).expect("the URL has segments");

        assert_eq!(segments.len(), 12);
    });

    // The spilled vector reserves sixteen 16-byte slots, the same 256 bytes the `Vec` allocated
    // on main — but it spills once instead of growing through two reallocations.
    assert_eq!(deep_stats.allocations, 1, "the inline buffer spills once");
    assert_eq!(deep_stats.reallocations, 0, "one spill, no growth");
    assert_eq!(deep_stats.bytes_allocated, 256, "sixteen 16-byte slots");
}
