//! The default coverage list and host pattern matching.
//!
//! The tested domains are a snapshot of unwall.app's own list (`GET /bootstrap`'s
//! `testedDomains`, mirrored by `GET /tested-domains`), refreshed from the live API in the
//! background. Until the first successful refresh — and whenever the API is unreachable — the
//! snapshot is what the plugin covers by default.

use std::time::Duration;

/// The tested domains of unwall.app, snapshotted on 2026-09-25.
const TESTED_DOMAINS_SNAPSHOT: &[&str] = &[
    "20minutes.fr", "20minutos.es", "404media.co", "abc.es",
    "abc.net.au", "abovethelaw.com", "aftonbladet.se", "airmail.news",
    "aljazeera.com", "allafrica.com", "apnews.com", "ara.cat",
    "arstechnica.com", "as.com", "asahi.com", "balkaninsight.com",
    "bangkokpost.com", "bbc.co.uk", "bild.de", "bizjournals.com",
    "boston.com", "bostonglobe.com", "businessinsider.com", "businesstoday.in",
    "buzzfeed.com", "buzzfeednews.com", "cadenaser.com", "capital.fr",
    "cbsnews.com", "chicagobusiness.com", "clarin.com", "cmjornal.pt",
    "cnbc.com", "cnn.com", "cope.es", "curbed.com",
    "dailymail.com", "dailysabah.com", "dailystar.co.uk", "dawn.com",
    "deadline.com", "derstandard.at", "diariocordoba.com", "diariodenavarra.es",
    "diariosur.es", "diariovasco.com", "discovermagazine.com", "dn.pt",
    "dn.se", "eater.com", "economist.com", "elcomercio.pe",
    "elconfidencial.com", "elcorreo.com", "eldiario.es", "eleconomista.es",
    "elespanol.com", "elindependiente.com", "elle.com", "elmercurio.com",
    "elmundo.es", "elnacional.com", "elpais.com", "elpais.com.uy",
    "elperiodico.com", "eluniversal.com.mx", "eluniverso.com", "engadget.com",
    "english.hani.co.kr", "espn.com", "eurogamer.net", "euronews.com",
    "expansion.com", "express.co.uk", "expresso.pt", "farodevigo.es",
    "fastcompany.com", "faz.net", "folha.uol.com.br", "foreignaffairs.com",
    "foxbusiness.com", "ft.com", "gaytimes.com", "gazzetta.it",
    "goettinger-tageblatt.de", "haaretz.com", "handelsblatt.com", "harpersbazaar.com",
    "henleystandard.co.uk", "heraldo.es", "hotnews.ro", "huffingtonpost.co.uk",
    "huffpost.com", "humanite.fr", "hurriyetdailynews.com", "idnes.cz",
    "ign.com", "ilfattoquotidiano.it", "ilsole24ore.com", "inc.com",
    "independent.co.uk", "irishnews.com", "irishtimes.com", "japantimes.co.jp",
    "koreaherald.com", "koreatimes.co.kr", "kyivpost.com", "lanacion.com.ar",
    "lanouvellerepublique.fr", "larazon.es", "larepublica.co", "lasprovincias.es",
    "lastampa.it", "latimes.com", "lavanguardia.com", "lavozdegalicia.es",
    "lebanonlocalnews.com", "lefigaro.fr", "lemonde.fr", "leparisien.fr",
    "lepoint.fr", "lequipe.fr", "lesechos.fr", "lesoir.be",
    "levante-emv.com", "liberation.fr", "libertaddigital.com", "lne.es",
    "marca.com", "marketwatch.com", "mediterraneodigital.com", "mensjournal.com",
    "metro.co.uk", "mg.co.za", "middleeasteye.net", "milenio.com",
    "mirror.co.uk", "modernhealthcare.com", "motherjones.com", "mundodeportivo.com",
    "naiz.eus", "nature.com", "newscientist.com", "notebookcheck.biz",
    "noticiasdenavarra.com", "nouvelobs.com", "noz.de", "npr.org",
    "nrc.nl", "nw.de", "nybooks.com", "nymag.com",
    "nypost.com", "nytimes.com", "nzz.ch", "observador.pt",
    "ouest-france.fr", "parade.com", "pcgamer.com", "politico.eu",
    "polygon.com", "popsci.com", "popularmechanics.com", "propublica.org",
    "publico.es", "quechoisir.org", "radiotimes.com", "repubblica.it",
    "romania-insider.com", "rotowire.com", "rte.ie", "sbnation.com",
    "scientific.net", "scientificamerican.com", "scmp.com", "semafor.com",
    "slate.com", "spiegel.de", "standard.co.uk", "startribune.com",
    "stuff.co.nz", "sudinfo.be", "sudouest.fr", "sueddeutsche.de",
    "tagesschau.de", "tarnkappe.info", "telegraph.co.uk", "theatlantic.com",
    "thebell.io", "thebulwark.com", "thecapitalistmag.com", "thecitizen.co.tz",
    "theconversation.com", "thecut.com", "thedailybeast.com", "thedispatch.com",
    "theglobeandmail.com", "theguardian.com", "thehill.com", "thehindu.com",
    "thehindubusinessline.com", "theintercept.com", "thejakartapost.com", "themercury.com.au",
    "themoscowtimes.com", "thenation.com", "thenationalnews.com", "thetimes.com",
    "theverge.com", "time.com", "timesofindia.indiatimes.com", "tomshardware.com",
    "towardsdatascience.com", "tribune.com.pk", "tuttosport.com", "usatoday.com",
    "usmagazine.com", "volkskrant.nl", "vulture.com", "washingtonpost.com",
    "watson.ch", "welt.de", "wired.com", "wyborcza.pl",
    "zeit.de", "zerohedge.com",
];

/// The delay before the first retry of a failed tested-domains refresh. Doubled for every
/// subsequent consecutive failure.
const REFRESH_BACKOFF_BASE: Duration = Duration::from_mins(1);

/// The maximum delay between retries of a failed tested-domains refresh.
const REFRESH_BACKOFF_MAX: Duration = Duration::from_hours(1);

/// The tested domains the plugin covers by default, until the first successful live refresh.
pub(super) fn default_sites() -> Vec<String> {
    TESTED_DOMAINS_SNAPSHOT
        .iter()
        .map(|site| (*site).to_owned())
        .collect()
}

/// The retry delay after `consecutive_failures` failed refreshes: [`REFRESH_BACKOFF_BASE`]
/// doubled per failure, capped at [`REFRESH_BACKOFF_MAX`].
#[must_use]
pub(super) fn refresh_backoff(consecutive_failures: u32) -> Duration {
    let doublings = consecutive_failures.saturating_sub(1).min(6);
    let delay = REFRESH_BACKOFF_BASE
        .checked_mul(1 << doublings)
        .unwrap_or(REFRESH_BACKOFF_MAX);

    delay.min(REFRESH_BACKOFF_MAX)
}

/// Whether `input` looks like a hostname: dot-separated labels of alphanumerics and hyphens,
/// none empty and none starting or ending with a hyphen.
fn is_hostname(input: &str) -> bool {
    if input.is_empty() {
        return false;
    }

    input.split('.').all(|label| {
        !label.is_empty()
            && label
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    })
}

/// Validates and normalizes a site argument: lowercased, an optional `*.` wildcard prefix, and
/// a hostname otherwise.
///
/// Returns [`None`] for anything else — paths, schemes, spaces, or empty labels.
#[must_use]
pub(super) fn normalize_site(input: &str) -> Option<String> {
    const WILDCARD: &str = "*.";

    let input = input.trim().to_lowercase();
    let wildcard = input.starts_with(WILDCARD);
    let host = input.strip_prefix(WILDCARD).unwrap_or(&input);

    is_hostname(host).then(|| {
        if wildcard {
            format!("{WILDCARD}{host}")
        } else {
            host.to_owned()
        }
    })
}

/// Whether `candidate` — a URL host or a stored site — matches `site`.
///
/// An exact entry matches the host and its `www.` variant; a `*.domain` wildcard matches the
/// bare domain and every subdomain.
#[must_use]
pub(super) fn matches_site(candidate: &str, site: &str) -> bool {
    let Some(domain) = site.strip_prefix("*.") else {
        return candidate.eq_ignore_ascii_case(site)
            || candidate
                .strip_prefix("www.")
                .is_some_and(|host| host.eq_ignore_ascii_case(site));
    };

    candidate
        .strip_suffix(domain)
        .is_some_and(|prefix| prefix.is_empty() || prefix.ends_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_sites_are_lowercase_hostnames() {
        assert!(!default_sites().is_empty());

        for site in default_sites() {
            assert_eq!(
                normalize_site(&site).as_deref(),
                Some(site.as_str()),
                "{site} should be a valid site entry"
            );
        }
    }

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(refresh_backoff(0), REFRESH_BACKOFF_BASE);
        assert_eq!(refresh_backoff(1), REFRESH_BACKOFF_BASE);
        assert_eq!(refresh_backoff(2), REFRESH_BACKOFF_BASE * 2);
        assert_eq!(refresh_backoff(3), REFRESH_BACKOFF_BASE * 4);
        assert_eq!(refresh_backoff(u32::MAX), REFRESH_BACKOFF_MAX);
    }

    #[test]
    fn normalizes_sites() {
        assert_eq!(
            normalize_site("  Bloomberg.COM ").as_deref(),
            Some("bloomberg.com")
        );
        assert_eq!(
            normalize_site("*.bloomberg.com").as_deref(),
            Some("*.bloomberg.com")
        );
        assert_eq!(normalize_site(""), None);
        assert_eq!(normalize_site("bloomberg.com/path"), None);
        assert_eq!(normalize_site("https://bloomberg.com"), None);
        assert_eq!(normalize_site("bloomberg .com"), None);
        assert_eq!(normalize_site("bloomberg..com"), None);
        assert_eq!(normalize_site("-bloomberg.com"), None);
    }

    #[test]
    fn sites_match_hosts() {
        assert!(matches_site("bloomberg.com", "bloomberg.com"));
        assert!(matches_site("BLOOMBERG.com", "bloomberg.com"));
        assert!(matches_site("www.bloomberg.com", "bloomberg.com"));
        assert!(!matches_site("api.bloomberg.com", "bloomberg.com"));

        // The wildcard matches the bare domain and every subdomain.
        assert!(matches_site("bloomberg.com", "*.bloomberg.com"));
        assert!(matches_site("www.bloomberg.com", "*.bloomberg.com"));
        assert!(matches_site("foo.bar.bloomberg.com", "*.bloomberg.com"));
        assert!(!matches_site("notbloomberg.com", "*.bloomberg.com"));
    }
}
