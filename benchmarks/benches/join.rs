//! Gungraun library benchmarks for joining list-like values into sentences.
//!
//! Run with `cargo bench -p zeta-benchmarks --bench join`.
//!
//! The benchmarks measure [`ListJoinExt`] and [`IteratorJoinExt`] against the standard
//! library's `join` as a baseline. Callgrind's `Ir` event tracks the instruction count of
//! every path, and DHAT additionally tracks the allocations: the slice path must stay at one
//! allocation per join, so any growth in `TotalBlocks` or `TotalBytes` is a regression.
//!
//! Save a reference baseline with `-- --save-baseline=initial` and compare against it with
//! `-- --baseline=initial`.

// The benchmark harness passes the inputs by value; borrowing them in the body is intended.
// Macro-generated items are benchmark scaffolding, not a public API, and their expanded
// paths trip the workspace lints.
#![allow(clippy::needless_pass_by_value, missing_docs, unused_qualifications)]

use std::hint::black_box;

use gungraun::prelude::*;
use gungraun::{Callgrind, Dhat, DhatMetric, EventKind, FlamegraphConfig};
use zeta_utils::text::{IteratorJoinExt, ListJoinExt};

/// The list size used by the item-length sweep.
const ITEM_LENGTH_LIST_SIZE: usize = 8;

/// Builds `size` string items of a realistic length.
fn items(size: usize) -> Vec<String> {
    Vec::from_fn(size, |index| format!("item-{index}"))
}

/// Builds a fixed-size list whose items are exactly `length` bytes.
fn items_with_length(length: usize) -> Vec<String> {
    Vec::from_fn(ITEM_LENGTH_LIST_SIZE, |index| format!("{index:0>length$}"))
}

#[library_benchmark]
#[bench::one_item(args = (1), setup = items)]
#[bench::two_items(args = (2), setup = items)]
#[bench::three_items(args = (3), setup = items)]
#[bench::eight_items(args = (8), setup = items)]
#[bench::sixty_four_items(args = (64), setup = items)]
fn bench_oxford_join_slice(items: Vec<String>) -> String {
    black_box(items.oxford_join())
}

#[library_benchmark]
#[bench::one_item(args = (1), setup = items)]
#[bench::two_items(args = (2), setup = items)]
#[bench::three_items(args = (3), setup = items)]
#[bench::eight_items(args = (8), setup = items)]
#[bench::sixty_four_items(args = (64), setup = items)]
fn bench_oxford_join_iterator(items: Vec<String>) -> String {
    black_box(items.iter().map(String::as_str).oxford_join())
}

#[library_benchmark]
#[bench::one_item(args = (1), setup = items)]
#[bench::two_items(args = (2), setup = items)]
#[bench::three_items(args = (3), setup = items)]
#[bench::eight_items(args = (8), setup = items)]
#[bench::sixty_four_items(args = (64), setup = items)]
fn bench_oxford_join_filtered_iterator(items: Vec<String>) -> String {
    black_box(
        items
            .iter()
            .filter(|item| item.len() > 2)
            .map(String::as_str)
            .oxford_join(),
    )
}

#[library_benchmark]
#[bench::one_item(args = (1), setup = items)]
#[bench::two_items(args = (2), setup = items)]
#[bench::three_items(args = (3), setup = items)]
#[bench::eight_items(args = (8), setup = items)]
#[bench::sixty_four_items(args = (64), setup = items)]
fn bench_join_with_separators(items: Vec<String>) -> String {
    black_box(items.join_with_separators(", ", " and "))
}

#[library_benchmark]
#[bench::one_item(args = (1), setup = items)]
#[bench::two_items(args = (2), setup = items)]
#[bench::three_items(args = (3), setup = items)]
#[bench::eight_items(args = (8), setup = items)]
#[bench::sixty_four_items(args = (64), setup = items)]
fn bench_std_join(items: Vec<String>) -> String {
    black_box(items.join(", "))
}

#[library_benchmark]
#[bench::eight_byte_items(args = (8), setup = items_with_length)]
#[bench::thirty_two_byte_items(args = (32), setup = items_with_length)]
#[bench::one_hundred_twenty_eight_byte_items(args = (128), setup = items_with_length)]
fn bench_oxford_join_item_lengths(items: Vec<String>) -> String {
    black_box(items.oxford_join())
}

library_benchmark_group!(
    name = join,
    compare_by_id = true,
    benchmarks = [
        bench_oxford_join_slice,
        bench_oxford_join_iterator,
        bench_oxford_join_filtered_iterator,
        bench_join_with_separators,
        bench_std_join,
        bench_oxford_join_item_lengths,
    ]
);

main!(
    config = LibraryBenchmarkConfig::default()
        .tool(
            Callgrind::default()
                .soft_limits([(EventKind::Ir, 1.0)])
                .flamegraph(FlamegraphConfig::default())
        )
        .tool(Dhat::default().soft_limits([
            (DhatMetric::TotalBlocks, 0.0),
            (DhatMetric::TotalBytes, 0.0),
        ])),
    library_benchmark_groups = join
);
