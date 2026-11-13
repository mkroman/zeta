//! The macros that expand the bundled-plugin list into the facade's surface.
//!
//! [`for_each_bundled_plugin!`] owns the list; the other macros each expand one item and are
//! only invoked through it, so the list stays the single source of truth. Splitting the
//! expansions keeps each one readable on its own: the re-exports and name list, the enum, the
//! `Plugin` forwarding and the `PluginSet` construction.
//!
//! These macros are internal to the crate. They expand to items that rely on names in scope at
//! the invocation site (the `BundledPlugin` enum and the plugin crates re-exported by
//! [`reexport_bundled_plugins!`]), so they are re-exported to the crate root with
//! `pub(crate) use` instead of `#[macro_export]`, per the reference's path-based macro scope.

/// Invokes `$expand` with the one list of bundled plugins.
///
/// Each entry names the cargo feature, the plugin crate and type, and the module alias (which
/// is also the plugin's configuration and catalog name):
///
/// ```text
/// "feature" => plugin_crate::PluginType as plugin_name
/// ```
///
/// Adding a bundled plugin is one row here plus its Cargo dependency, feature entries and TLS
/// forwarding. The rest of the facade expands from this row.
macro_rules! for_each_bundled_plugin {
    ($expand:ident) => {
        $expand! {
            "alert"             => plugins_alert::AlertPlugin as alert,
            "chaturbate"        => plugins_chaturbate::Chaturbate as chaturbate,
            "choices"           => plugins_choices::Choices as choices,
            "coinmarketcap"     => plugins_coinmarketcap::CoinMarketCap as coinmarketcap,
            "dendanskeordbog"   => plugins_dendanskeordbog::DenDanskeOrdbog as dendanskeordbog,
            "dig"               => plugins_dig::Dig as dig,
            "filter"            => plugins_filter::FilterPlugin as filter,
            "gay"               => plugins_gay::Gay as gay,
            "geoip"             => plugins_geoip::GeoIp as geoip,
            "github"            => plugins_github::GitHub as github,
            "health"            => plugins_health::Health as health,
            "help"              => plugins_help::Help as help,
            "howlongtobeat"     => plugins_howlongtobeat::HowLongToBeat as howlongtobeat,
            "imdb"              => plugins_imdb::Imdb as imdb,
            "instagram"         => plugins_instagram::Instagram as instagram,
            "isitopen"          => plugins_isitopen::IsItOpen as isitopen,
            "kagi"              => plugins_kagi::Kagi as kagi,
            "notification"      => plugins_notification::NotificationPlugin as notification,
            "ofn"               => plugins_ofn::Ofn as ofn,
            "openweathermap"    => plugins_openweathermap::OpenWeatherMap as openweathermap,
            "pornhub"           => plugins_pornhub::PornHub as pornhub,
            "reddit"            => plugins_reddit::Reddit as reddit,
            "rink"              => plugins_rink::Rink as rink,
            "rust-playground"   => plugins_rust_playground::RustPlayground as rust_playground,
            "spotify"           => plugins_spotify::Spotify as spotify,
            "string-utils"      => plugins_string_utils::StringUtils as string_utils,
            "thingiverse"       => plugins_thingiverse::Thingiverse as thingiverse,
            "tiktok"            => plugins_tiktok::Tiktok as tiktok,
            "titles"            => plugins_titles::Titles as titles,
            "trustpilot"        => plugins_trustpilot::Trustpilot as trustpilot,
            "tvmaze"            => plugins_tvmaze::Tvmaze as tvmaze,
            "twitch"            => plugins_twitch::Twitch as twitch,
            "unwall"            => plugins_unwall::Unwall as unwall,
            "urban-dictionary"  => plugins_urban_dictionary::UrbanDictionary as urban_dictionary,
            "youtube"           => plugins_youtube::YouTube as youtube,
        }
    };
}

pub(crate) use for_each_bundled_plugin;

/// Re-exports the compiled-in plugin crates and declares every bundled plugin's name.
///
/// The re-export keeps an optional dependency linked: an unreferenced optional crate is not
/// necessarily linked, which would silently drop its plugin. The name list includes plugins
/// whose feature is disabled, so configuration sections can distinguish a compiled-out plugin
/// from a typo.
macro_rules! reexport_bundled_plugins {
    ($($feature:literal => $plugin_crate:ident::$plugin_type:ident as $plugin_name:ident),+ $(,)?) => {
        $(
            #[cfg(feature = $feature)]
            pub use $plugin_crate as $plugin_name;
        )+

        /// The names of every bundled plugin, whether or not its cargo feature is enabled.
        ///
        /// Configuration sections for these names describe a plugin that was compiled out,
        /// not a typo, so the host can report the two cases differently.
        pub(crate) const BUNDLED_PLUGIN_NAMES: &[&str] = &[$(stringify!($plugin_name)),+];
    };
}

pub(crate) use reexport_bundled_plugins;

/// Declares [`BundledPlugin`], the compiled-in plugin set, and its name metadata.
///
/// One variant exists per plugin whose cargo feature is enabled. The runtime dispatches to the
/// concrete plugin type, so no plugin is boxed or type-erased.
macro_rules! define_bundled_plugin_enum {
    ($($feature:literal => $plugin_crate:ident::$plugin_type:ident as $plugin_name:ident),+ $(,)?) => {
        /// A compiled-in bundled plugin.
        ///
        /// One variant per plugin crate whose cargo feature is enabled; the runtime dispatches
        /// to the concrete type, so nothing is boxed or type-erased. Construction goes through
        /// the generated [`PluginSet`](::zeta_plugin::PluginSet) implementation.
        pub enum BundledPlugin {
            $(
                #[cfg(feature = $feature)]
                #[doc = concat!("The `", stringify!($plugin_name), "` plugin.")]
                $plugin_type($plugin_crate::$plugin_type),
            )+
        }

        impl BundledPlugin {
            /// The names of every compiled-in plugin, in declaration order.
            pub const NAMES: &'static [&'static str] = &[
                $(
                    #[cfg(feature = $feature)]
                    stringify!($plugin_name),
                )+
            ];

            /// Returns the plugin's name, as registered with [`Registered`](::zeta_plugin::Registered).
            #[must_use]
            #[allow(unreachable_patterns)]
            pub fn name(&self) -> &'static str {
                match self {
                    $(
                        #[cfg(feature = $feature)]
                        Self::$plugin_type(_) => {
                            <$plugin_crate::$plugin_type as ::zeta_plugin::Registered>::NAME
                        }
                    )+
                    _ => unreachable!("every BundledPlugin variant is compiled in"),
                }
            }
        }
    };
}

pub(crate) use define_bundled_plugin_enum;

/// Implements [`Plugin`](::zeta_plugin::Plugin) for [`BundledPlugin`] by forwarding to the
/// active variant.
///
/// One `async` block per call keeps every arm in the same future type; each plugin's future is
/// awaited on the stack instead of being boxed.
macro_rules! impl_plugin_for_bundled_plugin {
    ($($feature:literal => $plugin_crate:ident::$plugin_type:ident as $plugin_name:ident),+ $(,)?) => {
        #[allow(unreachable_patterns, unused_variables)]
        impl ::zeta_plugin::Plugin for BundledPlugin {
            fn handle<'a>(
                &'a self,
                ctx: &'a ::zeta_plugin::Context,
                client: &'a ::zeta_plugin::Client,
                event: &'a ::zeta_plugin::Event,
            ) -> impl ::core::future::Future<
                Output = ::core::result::Result<(), ::zeta_plugin::Error>,
            > + ::core::marker::Send
            + 'a {
                async move {
                    match self {
                        $(
                            #[cfg(feature = $feature)]
                            Self::$plugin_type(plugin) => {
                                ::zeta_plugin::Plugin::handle(plugin, ctx, client, event).await
                            }
                        )+
                        _ => ::core::result::Result::Ok(()),
                    }
                }
            }

            fn loaded<'a>(
                &'a mut self,
                ctx: &'a ::zeta_plugin::Context,
                client: &'a ::zeta_plugin::Client,
            ) -> impl ::core::future::Future<
                Output = ::core::result::Result<(), ::zeta_plugin::Error>,
            > + ::core::marker::Send
            + 'a {
                async move {
                    match self {
                        $(
                            #[cfg(feature = $feature)]
                            Self::$plugin_type(plugin) => {
                                ::zeta_plugin::Plugin::loaded(plugin, ctx, client).await
                            }
                        )+
                        _ => ::core::result::Result::Ok(()),
                    }
                }
            }

            fn shutdown<'a>(
                &'a mut self,
                ctx: &'a ::zeta_plugin::Context,
                client: &'a ::zeta_plugin::Client,
            ) -> impl ::core::future::Future<
                Output = ::core::result::Result<(), ::zeta_plugin::Error>,
            > + ::core::marker::Send
            + 'a {
                async move {
                    match self {
                        $(
                            #[cfg(feature = $feature)]
                            Self::$plugin_type(plugin) => {
                                ::zeta_plugin::Plugin::shutdown(plugin, ctx, client).await
                            }
                        )+
                        _ => ::core::result::Result::Ok(()),
                    }
                }
            }
        }
    };
}

pub(crate) use impl_plugin_for_bundled_plugin;

/// Implements [`PluginSet`](::zeta_plugin::PluginSet) for [`BundledPlugin`]: name lookup,
/// settings validation and construction.
macro_rules! impl_plugin_set_for_bundled_plugin {
    ($($feature:literal => $plugin_crate:ident::$plugin_type:ident as $plugin_name:ident),+ $(,)?) => {
        #[allow(unused_variables)]
        impl ::zeta_plugin::PluginSet for BundledPlugin {
            fn construct(
                ctx: &::zeta_plugin::Context,
                name: &str,
                section: ::core::option::Option<&::figment::value::Value>,
            ) -> ::core::result::Result<
                ::core::option::Option<::zeta_plugin::LoadedPlugin<Self>>,
                ::zeta_plugin::Error,
            > {
                $(
                    #[cfg(feature = $feature)]
                    if name == stringify!($plugin_name) {
                        let loaded = ::zeta_plugin::LoadedPlugin::<$plugin_crate::$plugin_type>::from_section(
                            ctx, section,
                        )?;

                        return ::core::result::Result::Ok(
                            ::core::option::Option::Some(
                                ::zeta_plugin::LoadedPlugin::map(loaded, Self::$plugin_type),
                            ),
                        );
                    }
                )+

                ::core::result::Result::Ok(::core::option::Option::None)
            }

            fn validate(
                name: &str,
                section: ::core::option::Option<&::figment::value::Value>,
            ) -> ::core::option::Option<
                ::core::result::Result<
                    ::std::vec::Vec<::std::string::String>,
                    ::zeta_plugin::Error,
                >,
            > {
                $(
                    #[cfg(feature = $feature)]
                    if name == stringify!($plugin_name) {
                        return ::core::option::Option::Some(
                            ::zeta_plugin::validate_settings::<$plugin_crate::$plugin_type>(section),
                        );
                    }
                )+

                ::core::option::Option::None
            }
        }
    };
}

pub(crate) use impl_plugin_set_for_bundled_plugin;
