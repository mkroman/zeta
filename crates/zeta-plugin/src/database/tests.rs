//! Unit tests for the database helpers: startup load retry and the reply-grade error contract.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use crate::Chain;

use super::*;

/// A failed load attempt that carries its attempt number.
#[derive(Debug)]
struct LoadError(u32);

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "attempt {}", self.0)
    }
}

impl std::error::Error for LoadError {}

/// A transient load failure is retried with a doubled delay: 250 ms, then 500 ms, before the
/// third attempt succeeds.
#[tokio::test(start_paused = true)]
async fn retries_a_transient_load_failure() {
    let attempts = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&attempts);
    let started = tokio::time::Instant::now();

    let result = load_with_retry(|| {
        let attempt = counter.fetch_add(1, Ordering::SeqCst) + 1;

        async move {
            if attempt < 3 {
                Err(LoadError(attempt))
            } else {
                Ok(())
            }
        }
    })
    .await;

    assert!(result.is_ok());
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    assert!(
        started.elapsed() >= Duration::from_millis(750),
        "250 ms + 500 ms of backoff, elapsed {:?}",
        started.elapsed()
    );
}

/// A persistent failure is returned only after every attempt, so the error logged by the
/// plugin task is the one from the last attempt.
#[tokio::test(start_paused = true)]
async fn a_persistent_load_failure_is_returned_after_every_attempt() {
    let attempts = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&attempts);
    let started = tokio::time::Instant::now();

    let result = load_with_retry(|| {
        let attempt = counter.fetch_add(1, Ordering::SeqCst) + 1;

        async move { Err::<(), _>(LoadError(attempt)) }
    })
    .await;

    assert_eq!(attempts.load(Ordering::SeqCst), LOAD_ATTEMPTS);
    assert_eq!(
        result.unwrap_err().to_string(),
        format!("attempt {LOAD_ATTEMPTS}")
    );
    assert!(
        started.elapsed() >= Duration::from_millis(1_750),
        "250 ms + 500 ms + 1 s of backoff, elapsed {:?}",
        started.elapsed()
    );
}

/// `Display` is the reply-grade sentence and the sqlx detail stays in the source chain.
#[test]
fn db_error_display_is_reply_grade_and_the_source_carries_the_detail() {
    let cases = [
        ("load", DbError::load("alert", sqlx::Error::RowNotFound)),
        ("insert", DbError::insert("alert", sqlx::Error::RowNotFound)),
        ("delete", DbError::delete("alert", sqlx::Error::RowNotFound)),
    ];

    for (stage, error) in cases {
        // A channel may see `Display`: it names the failure without the sqlx text.
        assert_eq!(error.to_string(), format!("could not {stage} alert"));

        // A field named `source` is the error's source, as the manual impl returned it.
        let source = std::error::Error::source(&error).expect("the sqlx error is the source");

        assert!(std::ptr::addr_eq(
            &raw const *source,
            &raw const error.source,
        ));

        // The operator log recovers the sqlx detail through the chain.
        assert!(
            Chain(&error)
                .to_string()
                .contains(&sqlx::Error::RowNotFound.to_string())
        );
    }
}
