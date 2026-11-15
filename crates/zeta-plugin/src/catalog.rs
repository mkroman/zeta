//! Metadata about the plugins registered with the bot.

use std::collections::HashSet;

use crate::{CommandSpec, UrlScope};

/// Metadata about a registered plugin.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct CatalogEntry {
    /// The name of the plugin.
    pub name: String,
    /// The commands handled by the plugin.
    pub commands: Vec<CommandSpec>,
    /// The scope of URLs the plugin registered interest in.
    pub url_scope: UrlScope,
}

impl CatalogEntry {
    /// Creates a new catalog entry.
    #[must_use]
    pub const fn new(name: String, commands: Vec<CommandSpec>, url_scope: UrlScope) -> Self {
        Self {
            name,
            commands,
            url_scope,
        }
    }

    /// Whether the plugin declares that it handles links posted on `host`.
    ///
    /// Only exact host registrations count: a plugin registered for every URL
    /// ([`UrlScope::Any`]) advertises no specific hosts and is therefore not a reason for a
    /// generic handler to skip a link.
    #[must_use]
    pub fn handles_host(&self, host: &str) -> bool {
        match self.url_scope {
            UrlScope::Hosts(hosts) => hosts
                .iter()
                .any(|handled| handled.eq_ignore_ascii_case(host)),
            UrlScope::None | UrlScope::Any => false,
        }
    }
}

/// Snapshot of the plugins registered with the bot and the events they handle.
///
/// The catalog is published to the context's shared state once registration is finished, so
/// plugins can look it up through [`Context::shared`](crate::Context::shared) to discover
/// other plugins. It is not replaced afterwards, so consumers may cache the handle.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct PluginCatalog {
    /// The registered plugins, in registration order.
    pub entries: Vec<CatalogEntry>,
    /// The lowercased hosts declared by [`UrlScope::Hosts`] entries, indexed for exact lookups.
    handled_hosts: HashSet<String>,
}

impl PluginCatalog {
    /// Creates a new catalog from `entries`, in registration order.
    ///
    /// The exact hosts the entries advertise are collected once here, so [`Self::handles_host`]
    /// does not walk every entry on each check.
    #[must_use]
    pub fn new(entries: Vec<CatalogEntry>) -> Self {
        let handled_hosts = entries
            .iter()
            .flat_map(|entry| match entry.url_scope {
                UrlScope::Hosts(hosts) => hosts,
                // `Any`, `None` and scopes added after this module was written advertise no
                // specific host; only exact host registrations count.
                _ => &[],
            })
            .map(|host| host.to_ascii_lowercase())
            .collect();

        Self {
            entries,
            handled_hosts,
        }
    }

    /// Whether any entry declares that it handles links posted on `host`.
    ///
    /// Hosts are matched exactly after ASCII-lowercasing both sides, mirroring
    /// [`CatalogEntry::handles_host`]. A plugin registered for every URL ([`UrlScope::Any`])
    /// advertises no specific hosts and is therefore not a reason for a generic handler to skip
    /// a link.
    #[must_use]
    pub fn handles_host(&self, host: &str) -> bool {
        self.handled_hosts.contains(host)
            || (host.bytes().any(|byte| byte.is_ascii_uppercase())
                && self.handled_hosts.contains(&host.to_ascii_lowercase()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns an entry with no commands, scoped to `url_scope`.
    fn entry(name: &str, url_scope: UrlScope) -> CatalogEntry {
        CatalogEntry::new(name.to_owned(), Vec::new(), url_scope)
    }

    #[test]
    fn indexes_exact_hosts_and_matches_them_case_insensitively() {
        let catalog = PluginCatalog::new(vec![
            entry("imdb", UrlScope::Hosts(&["imdb.com", "WWW.IMDB.COM"])),
            entry("youtube", UrlScope::Hosts(&["youtu.be"])),
        ]);

        assert!(catalog.handles_host("imdb.com"));
        assert!(catalog.handles_host("www.imdb.com"));
        assert!(catalog.handles_host("youtu.be"));
        assert!(catalog.handles_host("IMDB.com"));
        assert!(catalog.handles_host("WWW.IMDB.COM"));

        assert!(!catalog.handles_host("m.imdb.com"));
        assert!(!catalog.handles_host("example.com"));
        assert!(!catalog.handles_host(""));
    }

    #[test]
    fn generic_and_empty_scopes_advertise_no_hosts() {
        let catalog = PluginCatalog::new(vec![
            entry("titles", UrlScope::Any),
            entry("none", UrlScope::None),
        ]);

        assert!(!catalog.handles_host("example.com"));

        let empty = PluginCatalog::new(Vec::new());
        assert!(!empty.handles_host("example.com"));
    }

    #[test]
    fn catalog_and_entry_agree_on_handled_hosts() {
        let hosts = &["imdb.com", "www.imdb.com"];
        let catalog = PluginCatalog::new(vec![entry("imdb", UrlScope::Hosts(hosts))]);
        let entry = &catalog.entries[0];

        for host in ["imdb.com", "m.imdb.com", "example.com"] {
            assert_eq!(
                catalog.handles_host(host),
                entry.handles_host(host),
                "for {host}"
            );
        }
    }
}
