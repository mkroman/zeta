//! Tests for the alert service: scheduling and delivery, channel shutdown, pending listings,
//! the sync window and the repository deletion count.

use std::{cmp::Reverse, time::Duration};

use chrono::TimeDelta;
use sqlx::types::chrono::{DateTime, Utc};

use super::AlertService;
use crate::{NewAlert, Settings};
use zeta_plugin::database::Attribution;

/// A new alert fixture.
fn new_alert(nickname: &str, channel: &str, message: &str, time: DateTime<Utc>) -> NewAlert {
    NewAlert {
        attribution: Attribution::new(nickname, nickname, nickname, channel),
        message: message.into(),
        time,
    }
}

#[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]
async fn scheduler_delivers_created_alerts(db: sqlx::PgPool) {
    let mut service = AlertService::new(db.clone(), &Settings::default());
    let mut receiver = service.take_receiver().unwrap();

    service.load().await.unwrap();
    let _scheduler = service.start_scheduler();

    // The scheduler wakes through the stored notification for an alert created while its
    // cache is empty, so no settling delay is needed here.
    let now = Utc::now();
    let first = service
        .create(new_alert(
            "smoke",
            "#smoke",
            "first",
            now + TimeDelta::try_seconds(1).unwrap(),
        ))
        .await
        .unwrap();
    let second = service
        .create(new_alert(
            "smoke",
            "#smoke",
            "second",
            now + TimeDelta::try_seconds(3).unwrap(),
        ))
        .await
        .unwrap();

    // The alerts are delivered in order, roughly when they are due. Only the order is
    // asserted: a latency bound would flake on loaded machines.
    for expected in [&first, &second] {
        let delivered = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(delivered.id, expected.id);
    }

    // The alerts are deleted from the database once delivered. The deletion is not
    // synchronous with the delivery, so poll until it lands — with a deadline rather than
    // a fixed sleep, which would either flake on loaded machines or slow the happy path.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    loop {
        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM alerts WHERE id = ANY($1)")
            .bind([first.id, second.id])
            .fetch_one(&db)
            .await
            .unwrap();

        if remaining == 0 {
            break;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "the alerts were not deleted after delivery: {remaining} remaining"
        );

        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]
async fn closing_the_channel_drains_queued_alerts(db: sqlx::PgPool) {
    let mut service = AlertService::new(db, &Settings::default());
    let mut receiver = service.take_receiver().unwrap();

    let alert = service
        .create(new_alert("drain", "#smoke", "queued", Utc::now()))
        .await
        .unwrap();

    service.load().await.unwrap();
    service.scheduler.tick().await.unwrap();

    service.close();

    // The queued alert is still delivered, and the closed channel then ends the delivery
    // loop so its IRC sender is dropped.
    assert_eq!(receiver.recv().await.unwrap(), alert);
    assert_eq!(receiver.recv().await, None);
}

#[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]
async fn pending_for_filters_by_channel_and_nickname(db: sqlx::PgPool) {
    let service = AlertService::new(db, &Settings::default());

    let now = Utc::now();

    let sooner = service
        .create(new_alert(
            "lister",
            "#smoke",
            "sooner",
            now + TimeDelta::try_minutes(5).unwrap(),
        ))
        .await
        .unwrap();
    let later = service
        .create(new_alert(
            "lister",
            "#smoke",
            "later",
            now + TimeDelta::try_minutes(10).unwrap(),
        ))
        .await
        .unwrap();
    let _other = service
        .create(new_alert(
            "other",
            "#smoke",
            "other",
            now + TimeDelta::try_minutes(1).unwrap(),
        ))
        .await
        .unwrap();
    let _elsewhere = service
        .create(new_alert(
            "lister",
            "#other",
            "elsewhere",
            now + TimeDelta::try_minutes(1).unwrap(),
        ))
        .await
        .unwrap();

    let pending = service.pending_for("#smoke", "lister").await.unwrap();

    assert_eq!(pending, vec![sooner, later]);
}

#[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]
async fn sync_only_caches_alerts_within_the_window(db: sqlx::PgPool) {
    let service = AlertService::new(db, &Settings::default());

    let now = Utc::now();

    let inside = service
        .create(new_alert(
            "window",
            "#smoke",
            "inside",
            now + TimeDelta::try_minutes(5).unwrap(),
        ))
        .await
        .unwrap();
    let _outside = service
        .create(new_alert(
            "window",
            "#smoke",
            "outside",
            now + TimeDelta::try_hours(1).unwrap(),
        ))
        .await
        .unwrap();

    // The scheduler is not running, so the cache is only populated by the sync.
    service.load().await.unwrap();

    let cached = service.scheduler.cache.lock().await;

    assert_eq!(
        cached
            .iter()
            .filter(|Reverse(scheduled)| scheduled.0.nickname == "window")
            .map(|Reverse(scheduled)| scheduled.0.id)
            .collect::<Vec<_>>(),
        vec![inside.id]
    );

    drop(cached);
}

#[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]
async fn delete_ids_returns_the_removed_row_count(db: sqlx::PgPool) {
    let service = AlertService::new(db, &Settings::default());

    let alert = service
        .create(new_alert("counter", "#smoke", "hello", Utc::now()))
        .await
        .unwrap();

    assert_eq!(
        service
            .scheduler
            .repo
            .delete_ids(&[alert.id])
            .await
            .unwrap(),
        1
    );
    // A repeated delete removes nothing: the count reports rows, not the ids asked for.
    assert_eq!(
        service
            .scheduler
            .repo
            .delete_ids(&[alert.id])
            .await
            .unwrap(),
        0
    );
}
