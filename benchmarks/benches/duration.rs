//! Gungraun library benchmarks for duration formatting and calendar distances.
//!
//! Run with `cargo bench -p zeta-benchmarks --bench duration`.
//!
//! The calendar groups exercise the deterministic parts of the distance-in-words API
//! ([`DistanceInWordsExt`] for `chrono::DateTime<Utc>` and `time::OffsetDateTime`, and
//! [`TimeInWordsExt`] for `time::Duration`). Both implementations share their fixed date
//! cases, and the `compare_by_id` group option compares them against each other.
//!
//! The now-relative `in_words`/`time_ago`/`time_until` methods of points in time are
//! deliberately not benchmarked: they read the clock, which makes instruction counts
//! non-deterministic and unfit for regression checks. Their measurement core is covered by
//! `distance_in_words`; the wrappers only add a clock read and a clamp.
//!
//! Save a reference baseline with `-- --save-baseline=initial` and compare against it with
//! `-- --baseline=initial`.

// Macro-generated items are benchmark scaffolding, not a public API, and their expanded
// paths trip the workspace lints.
#![allow(missing_docs, unused_qualifications)]

use std::hint::black_box;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use gungraun::prelude::*;
use gungraun::{Callgrind, EventKind, FlamegraphConfig};
use time::{Date, Duration as TimeDuration, Month, OffsetDateTime};

use zeta_utils::duration::{
    DistanceInWordsExt, HOURS_AND_MINUTES, SECONDS_PER_DAY, SECONDS_PER_HOUR, SECONDS_PER_MINUTE,
    SECONDS_PER_WEEK, SECONDS_PER_YEAR, TimeInWordsExt, UNITS_WITH_SECONDS, format_duration,
    format_seconds_in_words, parse_iso8601_duration,
};

fn utc(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, hour, minute, second)
        .single()
        .expect("the benchmark date is valid")
}

fn offset_utc(
    year: i32,
    month: Month,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
) -> OffsetDateTime {
    Date::from_calendar_date(year, month, day)
        .expect("the benchmark date is valid")
        .with_hms(hour, minute, second)
        .expect("the benchmark time is valid")
        .assume_utc()
}

#[library_benchmark]
#[bench::zero_seconds(0_i64)]
#[bench::ninety_seconds(90)]
#[bench::two_hours_three_minutes(2 * SECONDS_PER_HOUR + 3 * SECONDS_PER_MINUTE)]
#[bench::three_days_four_hours(3 * SECONDS_PER_DAY + 4 * SECONDS_PER_HOUR)]
#[bench::one_year_one_week_one_day(SECONDS_PER_YEAR + SECONDS_PER_WEEK + SECONDS_PER_DAY)]
fn bench_format_seconds_full_table(seconds: i64) -> String {
    black_box(format_seconds_in_words(
        black_box(seconds),
        UNITS_WITH_SECONDS,
    ))
}

#[library_benchmark]
#[bench::two_hours_three_minutes(2 * SECONDS_PER_HOUR + 3 * SECONDS_PER_MINUTE)]
#[bench::one_hundred_hours(100 * SECONDS_PER_HOUR)]
fn bench_format_seconds_hours_and_minutes(seconds: i64) -> String {
    black_box(format_seconds_in_words(seconds, HOURS_AND_MINUTES))
}

#[library_benchmark]
#[bench::zero(Duration::ZERO)]
#[bench::forty_five_seconds(Duration::from_secs(45))]
#[bench::one_hour_two_minutes_twenty_seconds(
    Duration::from_hours(1) + Duration::from_secs(2 * 60 + 20)
)]
#[bench::one_day_two_hours_twenty_minutes_five_seconds(
    Duration::from_hours(25) + Duration::from_secs(20 * 60 + 5)
)]
fn bench_format_duration(duration: Duration) -> String {
    black_box(format_duration(black_box(duration)))
}

#[library_benchmark]
#[bench::pt45s("PT45S")]
#[bench::pt1h2m20s("PT1H2M20S")]
#[bench::p1dt2h20m5s("P1DT2H20M5S")]
fn bench_parse_iso8601(input: &'static str) -> Option<Duration> {
    black_box(parse_iso8601_duration(black_box(input)))
}

#[library_benchmark]
#[bench::equal_instants(utc(2024, 1, 1, 0, 0, 0), utc(2024, 1, 1, 0, 0, 0))]
#[bench::five_seconds(utc(2024, 1, 1, 0, 0, 0), utc(2024, 1, 1, 0, 0, 5))]
#[bench::six_hours_thirty_minutes(utc(2024, 1, 1, 0, 0, 0), utc(2024, 1, 1, 6, 30, 0))]
#[bench::end_of_month_clamp(utc(2024, 1, 31, 0, 0, 0), utc(2024, 2, 29, 0, 0, 0))]
#[bench::leap_day_anniversary(utc(2024, 2, 29, 0, 0, 0), utc(2025, 3, 1, 0, 0, 0))]
#[bench::fourteen_months(utc(2024, 1, 1, 12, 0, 0), utc(2025, 3, 2, 13, 30, 45))]
#[bench::fifty_five_years(utc(1969, 7, 20, 0, 0, 0), utc(2024, 7, 20, 0, 0, 0))]
fn bench_distance_in_words_chrono(from: DateTime<Utc>, to: DateTime<Utc>) -> String {
    black_box(from.distance_in_words(black_box(to)))
}

#[library_benchmark]
#[bench::equal_instants(
    offset_utc(2024, Month::January, 1, 0, 0, 0),
    offset_utc(2024, Month::January, 1, 0, 0, 0)
)]
#[bench::five_seconds(
    offset_utc(2024, Month::January, 1, 0, 0, 0),
    offset_utc(2024, Month::January, 1, 0, 0, 5)
)]
#[bench::six_hours_thirty_minutes(
    offset_utc(2024, Month::January, 1, 0, 0, 0),
    offset_utc(2024, Month::January, 1, 6, 30, 0)
)]
#[bench::end_of_month_clamp(
    offset_utc(2024, Month::January, 31, 0, 0, 0),
    offset_utc(2024, Month::February, 29, 0, 0, 0)
)]
#[bench::leap_day_anniversary(
    offset_utc(2024, Month::February, 29, 0, 0, 0),
    offset_utc(2025, Month::March, 1, 0, 0, 0)
)]
#[bench::fourteen_months(
    offset_utc(2024, Month::January, 1, 12, 0, 0),
    offset_utc(2025, Month::March, 2, 13, 30, 45)
)]
#[bench::fifty_five_years(
    offset_utc(1969, Month::July, 20, 0, 0, 0),
    offset_utc(2024, Month::July, 20, 0, 0, 0)
)]
fn bench_distance_in_words_time(from: OffsetDateTime, to: OffsetDateTime) -> String {
    black_box(from.distance_in_words(black_box(to)))
}

#[library_benchmark]
#[bench::five_minutes(TimeDuration::minutes(5))]
#[bench::four_hundred_days(TimeDuration::days(400))]
fn bench_time_duration_in_words(duration: TimeDuration) -> String {
    black_box(duration.in_words())
}

library_benchmark_group!(
    name = duration,
    compare_by_id = true,
    benchmarks = [
        bench_format_seconds_full_table,
        bench_format_seconds_hours_and_minutes,
        bench_format_duration,
        bench_parse_iso8601,
        bench_distance_in_words_chrono,
        bench_distance_in_words_time,
        bench_time_duration_in_words,
    ]
);

main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .soft_limits([(EventKind::Ir, 1.0)])
            .flamegraph(FlamegraphConfig::default())
    ),
    library_benchmark_groups = duration
);
