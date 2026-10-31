//! Heap-allocation regression test for [`path_segments`].
//!
//! `allocation-counter` tallies allocations per thread, so this test runs alongside the rest of
//! the suite without interference. Linking the crate installs its counting allocator; the test
//! needs no `#[global_allocator]` of its own.

use std::hint::black_box;

use url::Url;

use zeta_url::path_segments;

/// Returns the allocation statistics `f` performs on this thread.
fn measure(f: impl FnOnce()) -> allocation_counter::AllocationInfo {
    allocation_counter::measure(f)
}

#[test]
fn path_segments_keeps_typical_urls_off_the_heap() {
    let typical = Url::parse("https://www.example.com/a/b").expect("valid URL");

    let info = measure(|| {
        let segments = path_segments(&typical).expect("the URL has segments");

        assert_eq!(segments.as_slice(), ["a", "b"]);
        black_box(&segments);
    });

    // On main this collected into a `Vec<&str>`: one allocation of 64 bytes (a two-element
    // vector reserves four 16-byte slots).
    assert_eq!(info.count_total, 0, "the segments live inline");
    assert_eq!(info.bytes_total, 0, "nothing reaches the heap");
    assert_eq!(info.bytes_max, 0);

    // Six segments — a reddit comment permalink — still fit inline.
    let six = Url::parse("https://www.example.com/a/b/c/d/e/f").expect("valid URL");

    let info = measure(|| {
        let segments = path_segments(&six).expect("the URL has segments");

        assert_eq!(segments.len(), 6);
        black_box(&segments);
    });

    assert_eq!(info.count_total, 0, "six segments live inline");
    assert_eq!(info.bytes_total, 0, "nothing reaches the heap");
    assert_eq!(info.bytes_max, 0);

    // Twelve segments overflow the inline buffer: one allocation spills it to eight slots, then
    // the grow reallocates to sixteen. `allocation-counter` does not override `realloc`, so the
    // default implementation counts the grow as a second allocation operation (allocate the new
    // buffer, copy, free the old one) and adds both requested buffers to the byte total.
    let deep = Url::parse("https://www.example.com/a/b/c/d/e/f/g/h/i/j/k/l").expect("valid URL");

    let info = measure(|| {
        let segments = path_segments(&deep).expect("the URL has segments");

        assert_eq!(segments.len(), 12);
        black_box(&segments);
    });

    assert_eq!(
        info.count_total, 2,
        "one spill allocation plus the reallocation"
    );
    assert_eq!(
        info.bytes_total, 384,
        "128 B for eight slots, then 256 B for sixteen"
    );
    assert_eq!(
        info.bytes_max, 384,
        "the old and new buffers are both live during the grow"
    );
}
