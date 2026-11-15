//! Lifecycle, URL filtering and fetch concurrency tests for the titles plugin.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

use zeta_plugin::{CatalogEntry, PluginCatalog, UrlScope};
use zeta_test_support::{assert_empty, settings_tests};

use super::fetch::{
    ends_with_ignore_ascii_case, is_binary_url, is_catalog_handled, is_ignored_host, should_ignore,
};
use super::*;

/// Starts a minimal HTTP server that answers each request after `delay`, returning its
/// address, the most connections it ever had open at once, and the number it answered.
///
/// A connection counts as open from the moment it is accepted until its response is started,
/// so two fetches that overlap show up as two open connections; answering the request is no
/// longer part of the overlap window.
async fn delayed_page_server(delay: Duration) -> (SocketAddr, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the test server");
    let address = listener.local_addr().expect("the server has an address");
    let open = Arc::new(AtomicUsize::new(0));
    let most_open = Arc::new(AtomicUsize::new(0));
    let answered = Arc::new(AtomicUsize::new(0));

    tokio::spawn({
        let open = Arc::clone(&open);
        let most_open = Arc::clone(&most_open);
        let answered = Arc::clone(&answered);

        async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let open = Arc::clone(&open);
                let most_open = Arc::clone(&most_open);
                let answered = Arc::clone(&answered);

                tokio::spawn(async move {
                    let now_open = open.fetch_add(1, Ordering::SeqCst) + 1;
                    most_open.fetch_max(now_open, Ordering::SeqCst);

                    tokio::time::sleep(delay).await;

                    let mut request = [0_u8; 1024];
                    let _ = stream.read(&mut request).await;

                    // Leave the overlap window before the response can release the fetch's
                    // permit, so a fetch that starts next never overlaps this one.
                    open.fetch_sub(1, Ordering::SeqCst);

                    let body = "<html><head><title>Hello</title></head></html>";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );

                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                    answered.fetch_add(1, Ordering::SeqCst);
                });
            }
        }
    });

    (address, most_open, answered)
}

/// Returns a catalog whose only entry declares `hosts`.
fn catalog_with(name: &str, hosts: &'static [&'static str]) -> PluginCatalog {
    PluginCatalog::new(vec![CatalogEntry::new(
        name.to_owned(),
        Vec::new(),
        UrlScope::Hosts(hosts),
    )])
}

#[test]
fn ignores_command_messages() {
    assert!(should_ignore(".gis https://maero.dk"));
    assert!(should_ignore(".ofn"));
    assert!(should_ignore(".help"));

    assert!(!should_ignore("a .dot in a sentence https://maero.dk"));
    assert!(!should_ignore("... https://maero.dk"));
}

#[test]
fn ignores_bot_replies() {
    // CTCP messages never reach the plugin at all: they are their own event kind.
    assert!(should_ignore("\x0310> Some page"));
    assert!(should_ignore("\x0310>\x0f\x02 Site:\x02\x0310 Some page"));

    assert!(!should_ignore("check this out https://maero.dk"));
}

#[tokio::test]
async fn caches_the_catalog_on_load() {
    let ctx = zeta_test_support::test_context();
    ctx.shared().publish(Arc::new(catalog_with(
        "imdb",
        &["imdb.com", "www.imdb.com"],
    )));

    let mut titles = Titles::new(&ctx, &Settings::default()).expect("titles plugin builds");
    let client = zeta_test_support::mock_client().await;

    titles.loaded(&ctx, &client).await.expect("plugin loads");

    // A catalog published after the load hook does not replace the cached one.
    ctx.shared().publish(Arc::new(catalog_with(
        "youtube",
        &["youtube.com", "youtu.be"],
    )));

    let catalog = titles
        .catalog(&ctx)
        .expect("the catalog is cached during load");

    assert!(is_catalog_handled(
        &Url::parse("https://www.imdb.com/title/tt1375666").unwrap(),
        Some(catalog)
    ));
    assert!(!is_catalog_handled(
        &Url::parse("https://youtu.be/dQw4w9WgXcQ").unwrap(),
        Some(catalog)
    ));
}

#[tokio::test]
async fn bounds_concurrent_fetches() {
    let ctx = zeta_test_support::test_context();
    let mut titles = Titles::new(&ctx, &Settings::default()).expect("titles plugin builds");

    // One permit: a second fetch may only start once the first one is done.
    titles.fetches = Arc::new(Semaphore::new(1));

    let client = zeta_test_support::mock_client().await;

    let (address, most_open, answered) = delayed_page_server(Duration::from_millis(300)).await;

    let urls = ["/one", "/two"]
        .map(|path| Arc::new(Url::parse(&format!("http://{address}{path}")).expect("valid url")));

    tokio::join!(
        titles.process_url(Arc::clone(&urls[0]), "#test", &client),
        titles.process_url(Arc::clone(&urls[1]), "#test", &client),
    );

    tokio::time::timeout(Duration::from_secs(5), async {
        while answered.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("both fetches finish");

    assert_eq!(
        most_open.load(Ordering::SeqCst),
        1,
        "more than one fetch was in flight at a time"
    );
}

#[tokio::test]
async fn resolves_an_uncached_catalog_from_shared_state() {
    let ctx = zeta_test_support::test_context();
    ctx.shared().publish(Arc::new(catalog_with(
        "imdb",
        &["imdb.com", "www.imdb.com"],
    )));

    // The load hook never runs when a test calls the handler directly.
    let titles = Titles::new(&ctx, &Settings::default()).expect("titles plugin builds");

    let catalog = titles
        .catalog(&ctx)
        .expect("the catalog is resolved from shared state");

    assert!(is_catalog_handled(
        &Url::parse("https://www.imdb.com/title/tt1375666").unwrap(),
        Some(catalog)
    ));

    // The fallback is cached too, so later events do not consult shared state again.
    ctx.shared().publish(Arc::new(catalog_with(
        "youtube",
        &["youtube.com", "youtu.be"],
    )));

    let cached = titles.catalog(&ctx).expect("the catalog is cached");

    assert!(!is_catalog_handled(
        &Url::parse("https://youtu.be/dQw4w9WgXcQ").unwrap(),
        Some(cached)
    ));
}

#[tokio::test]
async fn has_no_catalog_until_one_is_published() {
    let ctx = zeta_test_support::test_context();
    let titles = Titles::new(&ctx, &Settings::default()).expect("titles plugin builds");

    assert!(titles.catalog(&ctx).is_none());
}

#[test]
fn ignores_configured_hosts() {
    let settings = Settings {
        ignored_hosts: vec![
            "WWW.REDDIT.COM".to_string(),
            "youtu.be".to_string(),
            "vm.tiktok.com".to_string(),
        ],
        ..Settings::default()
    };

    assert!(is_ignored_host(
        &Url::parse("https://www.reddit.com/r/rust").unwrap(),
        &settings.ignored_hosts
    ));
    assert!(is_ignored_host(
        &Url::parse("https://youtu.be/abc").unwrap(),
        &settings.ignored_hosts
    ));
    assert!(is_ignored_host(
        &Url::parse("https://vm.tiktok.com/abc").unwrap(),
        &settings.ignored_hosts
    ));

    assert!(!is_ignored_host(
        &Url::parse("https://maero.dk").unwrap(),
        &settings.ignored_hosts
    ));
}

#[test]
fn ignores_hosts_declared_in_the_catalog() {
    let catalog = PluginCatalog::new(vec![
        CatalogEntry::new(
            "imdb".to_owned(),
            Vec::new(),
            UrlScope::Hosts(&["imdb.com", "WWW.IMDB.COM"]),
        ),
        // Generic handlers advertise no specific hosts.
        CatalogEntry::new("titles".to_owned(), Vec::new(), UrlScope::Any),
        CatalogEntry::new("none".to_owned(), Vec::new(), UrlScope::None),
    ]);
    assert!(is_catalog_handled(
        &Url::parse("https://www.imdb.com/title/tt1375666").unwrap(),
        Some(&catalog)
    ));
    assert!(is_catalog_handled(
        &Url::parse("https://IMDB.com/title/tt1375666").unwrap(),
        Some(&catalog)
    ));
    assert!(!is_catalog_handled(
        &Url::parse("https://maero.dk").unwrap(),
        Some(&catalog)
    ));
    assert!(!is_catalog_handled(
        &Url::parse("https://example.com").unwrap(),
        Some(&catalog)
    ));
    assert!(!is_catalog_handled(
        &Url::parse("https://maero.dk").unwrap(),
        None
    ));
}

#[test]
fn ignores_binary_urls() {
    assert!(is_binary_url(
        &Url::parse("https://maero.dk/photo.JPG").unwrap()
    ));
    assert!(is_binary_url(
        &Url::parse("https://maero.dk/photo.Jpg").unwrap()
    ));
    assert!(is_binary_url(
        &Url::parse("https://maero.dk/archive.tar.gz").unwrap()
    ));
    assert!(is_binary_url(
        &Url::parse("https://maero.dk/video.mp4?start=30").unwrap()
    ));
    assert!(is_binary_url(
        &Url::parse("https://maero.dk/photo.JPG/").unwrap()
    ));
    // Trailing slashes are skipped, so the extension is still found after a double slash.
    assert!(is_binary_url(
        &Url::parse("https://maero.dk/photo.JPG//").unwrap()
    ));

    // Extensionless paths and the root path are pages, not binaries.
    assert!(!is_binary_url(
        &Url::parse("https://maero.dk/page.html").unwrap()
    ));
    assert!(!is_binary_url(&Url::parse("https://maero.dk").unwrap()));
    assert!(!is_binary_url(&Url::parse("https://maero.dk/").unwrap()));
    assert!(!is_binary_url(
        &Url::parse("https://maero.dk/photo").unwrap()
    ));
}

#[test]
fn compares_extension_suffixes_without_allocating() {
    assert!(ends_with_ignore_ascii_case("photo.JPG", ".jpg"));
    assert!(ends_with_ignore_ascii_case("photo.jpg", ".JPG"));
    assert!(ends_with_ignore_ascii_case("archive.tar.gz", ".tar.gz"));
    assert!(!ends_with_ignore_ascii_case("photo.jpeg", ".jpg"));
    // A needle longer than the haystack never matches, and must not panic.
    assert!(!ends_with_ignore_ascii_case("", ".jpg"));
    assert!(!ends_with_ignore_ascii_case(".jp", ".jpg"));
}

settings_tests! {
    Settings,
    settings,
    default: {
        assert_empty!(settings.ignored_hosts);
        assert_eq!(settings.max_redirects, 3);
        assert_eq!(settings.max_message_length, 400);
        assert_eq!(settings.max_description_length, 200);
    }
    deserialize: {
        "ignored_hosts": ["example.com"],
        "max_redirects": 1,
        "max_message_length": 100,
        "max_description_length": 50,
    } assert: {
        assert_eq!(settings.ignored_hosts, ["example.com"]);
        assert_eq!(settings.max_redirects, 1);
        assert_eq!(settings.max_message_length, 100);
        assert_eq!(settings.max_description_length, 50);
    }
}
