//! Heap-allocation regression test for [`path_segments`].
//!
//! `allocation-counter` tallies allocations per thread, so this test runs alongside the rest of
//! the suite without interference. Linking the crate installs its counting allocator; the test
//! needs no `#[global_allocator]` of its own.

use std::hint::black_box;
use std::mem::size_of;

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

    // Twelve segments overflow the inline buffer, so the buffer must spill to the heap. The exact
    // growth policy belongs to `SmallVec`; assert only that the spill happens and reserves room
    // for all twelve borrowed segments, so the test does not pin dependency internals.
    let deep = Url::parse("https://www.example.com/a/b/c/d/e/f/g/h/i/j/k/l").expect("valid URL");

    let info = measure(|| {
        let segments = path_segments(&deep).expect("the URL has segments");

        assert_eq!(segments.len(), 12);
        black_box(&segments);
    });

    let required = u64::try_from(12 * size_of::<&str>())
        .expect("the inline buffer size fits in the allocation counter's u64");

    assert!(info.count_total > 0, "the deep path must spill to the heap");
    assert!(
        info.bytes_total >= required,
        "the spilled buffer holds twelve references"
    );
    assert!(
        info.bytes_max >= required,
        "the peak allocation holds twelve references"
    );
}
