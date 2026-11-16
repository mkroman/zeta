//! Tests for the unwall service: the covered-site set, removal and the cached submission path.

use std::borrow::Cow;
use std::time::Duration;

use url::Url;

use super::{UnwallService, lowercase_host};
use crate::client::UnwallClient;
use crate::model::NewFetch;
use zeta_http as http;
use zeta_http::HttpConfig;
use zeta_plugin::database::{Attribution, Database};
use zeta_test_support::assert_empty;
use zeta_test_support::wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, query_param},
};

/// A service against an empty database and an unreachable API, for the paths that never
/// submit.
fn db_service(db: Database) -> UnwallService {
    let client = UnwallClient::new(
        http::build_client(&HttpConfig::default()),
        Duration::from_secs(60),
        "http://127.0.0.1:9".parse().unwrap(),
    );

    UnwallService::new(db, client, "https://unwall.app/".parse().unwrap())
}

/// The attribution of a fetch posted by `mk`.
fn fetch(url: &Url) -> NewFetch {
    NewFetch {
        url: url.as_str().to_owned(),
        host: url.host_str().unwrap_or_default().to_owned(),
        attribution: Attribution::new("mk", "~mk", "user.example", "#news"),
        network_id: "irc.rwx.im:6697".into(),
    }
}

#[test]
fn lowercases_hosts_only_when_needed() {
    // A lowercase host is borrowed, so the lookup stays allocation-free.
    assert!(matches!(lowercase_host("example.com"), Cow::Borrowed(_)));
    assert_eq!(lowercase_host("WWW.Bloomberg.com"), "www.bloomberg.com");
}

#[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]
async fn nothing_is_covered_before_an_admin_adds_a_site(db: sqlx::PgPool) {
    let service = db_service(db);
    service.load().await.unwrap();

    // Neither a site unwall.app tests nor any other host is covered out of the box.
    assert!(!service.covers("ft.com"));
    assert!(!service.covers("www.ft.com"));
    assert!(!service.covers("example.com"));
    assert_empty!(service.sites());
}

#[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]
async fn added_sites_cover_their_host_www_variant_and_wildcards(db: sqlx::PgPool) {
    let service = db_service(db);
    service.load().await.unwrap();

    service
        .add_sites(&["bloomberg.com".to_owned()], "admin")
        .await
        .unwrap();

    assert!(service.covers("bloomberg.com"));
    assert!(service.covers("www.bloomberg.com"));
    assert!(!service.covers("api.bloomberg.com"));

    // Direct callers may pass the host in any ASCII case.
    assert!(service.covers("BLOOMBERG.COM"));
    assert!(service.covers("WWW.Bloomberg.com"));

    service
        .add_sites(&["*.bloomberg.com".to_owned()], "admin")
        .await
        .unwrap();

    assert!(service.covers("api.bloomberg.com"));

    // The added sites survive a fresh load: the database is the source of truth.
    service.load().await.unwrap();

    assert!(service.covers("api.bloomberg.com"));
}

#[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]
async fn removing_matches_the_added_sites_and_survives_a_reload(db: sqlx::PgPool) {
    let service = db_service(db);
    service.load().await.unwrap();

    service
        .add_sites(
            &["bloomberg.com".to_owned(), "*.bloomberg.com".to_owned()],
            "admin",
        )
        .await
        .unwrap();

    let removed = service.remove_matching("*.bloomberg.com").await.unwrap();

    assert_eq!(removed, ["*.bloomberg.com", "bloomberg.com"]);
    assert!(!service.covers("www.bloomberg.com"));
    assert!(!service.covers("bloomberg.com"));
    assert_empty!(service.sites());

    // The removal survives a fresh load, and re-adding brings the host back.
    service.load().await.unwrap();

    assert!(!service.covers("bloomberg.com"));

    service
        .add_sites(&["bloomberg.com".to_owned()], "admin")
        .await
        .unwrap();

    assert!(service.covers("bloomberg.com"));
}

#[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]
async fn removing_without_a_match_removes_nothing(db: sqlx::PgPool) {
    let service = db_service(db);
    service.load().await.unwrap();

    service
        .add_sites(&["bloomberg.com".to_owned()], "admin")
        .await
        .unwrap();

    let removed = service.remove_matching("*.ft.com").await.unwrap();

    assert_empty!(removed);
    assert!(service.covers("bloomberg.com"));
}

#[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]
async fn resolve_submits_once_then_answers_from_the_cache(db: sqlx::PgPool) {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/fetch"))
        .and(query_param("url", "https://www.bloomberg.com/news/a"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let client = UnwallClient::new(
        http::build_client(&HttpConfig::default()),
        Duration::from_secs(60),
        crate::client::base_url(&server.uri()).unwrap(),
    );
    let service = UnwallService::new(db, client, "https://unwall.app/".parse().unwrap());

    let mut article: Url = "https://www.bloomberg.com/news/a#frag".parse().unwrap();
    article.set_fragment(None);

    let first = service.resolve(&article, fetch(&article)).await.unwrap();
    assert_eq!(
        first.unwall_url,
        "https://unwall.app/www.bloomberg.com/news/a"
    );

    let second = service.resolve(&article, fetch(&article)).await.unwrap();
    assert_eq!(first, second);

    // Exactly one fetch was recorded, attributed to one user.
    let stats = service.statistics().await.unwrap();

    assert_eq!(stats.urls, 1);
    assert_eq!(stats.hosts, 1);
    assert_eq!(stats.fetches, 1);
    assert_eq!(stats.users, 1);

    let host_stats = service
        .host_statistics("WWW.Bloomberg.com")
        .await
        .unwrap()
        .expect("the host has been unwalled");

    assert_eq!(host_stats.urls, 1);
    assert_eq!(host_stats.users, 1);
    assert!(host_stats.latest.is_some());

    assert!(
        service
            .host_statistics("example.com")
            .await
            .unwrap()
            .is_none()
    );
}
