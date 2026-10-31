//! Heap-allocation regression test for [`path_segments`].
//!
//! The allocator counters are process-wide, so both cases live in a single test: tests in one
//! binary run in parallel, and a second test allocating between the snapshots would inflate the
//! counts.

use std::alloc::System;

use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use url::Url;

use zeta_url::path_segments;

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

/// Returns how many allocations `f` performs.
fn allocations(f: impl FnOnce()) -> usize {
    let mut region = Region::new(GLOBAL);

    f();

    region.change_and_reset().allocations
}

#[test]
fn path_segments_keeps_typical_urls_off_the_heap() {
    let typical = Url::parse("https://www.example.com/a/b").expect("valid URL");

    let typical_allocations = allocations(|| {
        let segments = path_segments(&typical).expect("the URL has segments");

        assert_eq!(segments.as_slice(), ["a", "b"]);
    });

    assert_eq!(typical_allocations, 0, "the segments live inline");

    // Twelve segments overflow the inline buffer, which spills to the heap exactly once.
    let deep = Url::parse("https://www.example.com/a/b/c/d/e/f/g/h/i/j/k/l").expect("valid URL");

    let deep_allocations = allocations(|| {
        let segments = path_segments(&deep).expect("the URL has segments");

        assert_eq!(segments.len(), 12);
    });

    assert_eq!(deep_allocations, 1, "the inline buffer spills once");
}
