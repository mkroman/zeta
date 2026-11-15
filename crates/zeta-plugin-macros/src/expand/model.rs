//! The classified contents of a `#[plugin]` impl block.
//!
//! Classification builds these values from the block's annotations and validation rejects
//! anything the renderer could not implement, so the types themselves only hold the result.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::{Ident, Type};

use crate::args::{CommandArgs, UrlsArgs};

/// The classified contents of a `#[plugin]` impl block.
pub(super) struct Plugin {
    /// The plugin's name, from the required `name = "…"` argument.
    pub(super) name: syn::LitStr,
    /// The plugin's settings type, or `None` for `NoSettings`.
    pub(super) settings: Option<Type>,
    /// The URL registration, if any.
    pub(super) urls: Option<UrlsArgs>,
    /// Whether every channel message is received.
    pub(super) messages: bool,
    /// The inherent constructor, when the impl block declared one.
    pub(super) constructor: Option<Constructor>,
    /// The `#[command]` methods, in declaration order.
    pub(super) commands: Vec<Command>,
    /// The `#[handler(kind)]` methods, in declaration order.
    pub(super) handlers: Vec<Handler>,
}

/// The inherent plugin constructor.
pub(super) struct Constructor {
    /// The recognized parameters, in declaration order.
    pub(super) parameters: Vec<ConstructorParameter>,
}

impl Constructor {
    /// Whether the constructor declares the parameter's kind.
    pub(super) fn takes(&self, parameter: ConstructorParameter) -> bool {
        self.parameters.contains(&parameter)
    }

    /// The arguments the generated constructor forwards, in declaration order.
    pub(super) fn arguments(&self) -> TokenStream {
        let arguments = self.parameters.iter().map(|parameter| match parameter {
            ConstructorParameter::Context => quote!(ctx),
            ConstructorParameter::Settings => quote!(settings),
            ConstructorParameter::Subscriptions => quote!(subscriptions),
        });

        quote!(#(#arguments),*)
    }
}

/// One parameter of the inherent plugin constructor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ConstructorParameter {
    /// `&Context`.
    Context,
    /// An immutable reference to the plugin's settings type.
    Settings,
    /// `&mut Subscriptions`.
    Subscriptions,
}

impl ConstructorParameter {
    /// How the parameter is written in diagnostics.
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Context => "&Context",
            Self::Settings => "an immutable reference to the plugin's settings",
            Self::Subscriptions => "&mut Subscriptions",
        }
    }
}

/// One `#[command]` method.
pub(super) struct Command {
    /// The module-level `CommandSpec` constant the command is registered with.
    pub(super) spec: Ident,
    /// The parsed attribute arguments.
    pub(super) args: CommandArgs,
    /// The inherent method implementing the command.
    pub(super) method: Ident,
    /// Whether the method declares the optional `channel: &str` parameter, which only event
    /// commands may omit.
    pub(super) channel: bool,
    /// The method's `#[cfg]`/`#[cfg_attr]` attributes, copied to everything generated for it.
    pub(super) cfg: Vec<syn::Attribute>,
}

/// One `#[handler(kind)]` method.
pub(super) struct Handler {
    /// The event kind the method handles.
    pub(super) kind: HandlerKind,
    /// The inherent method handling the event.
    pub(super) method: Ident,
    /// The span of the `#[handler]` attribute, for conflict errors.
    pub(super) span: Span,
    /// The method's `#[cfg]`/`#[cfg_attr]` attributes, copied to the forwarding method.
    pub(super) cfg: Vec<syn::Attribute>,
}

/// Returns `word` with its first character uppercased, for the `KEvent` type names.
fn pascal_case(word: &str) -> String {
    let mut characters = word.chars();

    characters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(characters).collect()
    })
}

/// Generates [`HandlerKind`] from one table of keywords.
///
/// Every event kind follows a single convention: the keyword `k` names the `Event::K` variant
/// and the `KEvent` parameter type, and the generated `Plugin::handle` match routes that variant
/// to the annotated method. The lifecycle hooks take no event, which the table marks as such
/// instead of naming their event type separately.
macro_rules! handler_kinds {
    ($($variant:ident($keyword:literal, $role:ident)),* $(,)?) => {
        /// The event kinds a `#[handler(kind)]` method can handle.
        #[derive(Clone, Copy, PartialEq)]
        pub(super) enum HandlerKind {
            $($variant,)*
        }

        impl HandlerKind {
            /// Every kind, in the order diagnostics list them.
            const ALL: &[Self] = &[$(Self::$variant,)*];

            /// Parses the `kind` argument of `#[handler(kind)]`.
            pub(super) fn from_ident(ident: &Ident) -> Option<Self> {
                Self::ALL.iter().copied().find(|kind| ident == kind.name())
            }

            /// The error for an unrecognized `kind`, listing the accepted ones.
            pub(super) fn unknown_kind(ident: &Ident) -> syn::Error {
                let expected = Self::ALL
                    .iter()
                    .map(|kind| format!("`{}`", kind.name()))
                    .collect::<Vec<_>>()
                    .join(", ");

                syn::Error::new(
                    ident.span(),
                    format!("unknown event kind `{ident}`; expected one of {expected}"),
                )
            }

            /// The kind as written in `#[handler(kind)]`, for diagnostics.
            pub(super) const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $keyword,)*
                }
            }

            /// Whether this is an eventless lifecycle hook rather than an event.
            pub(super) const fn is_lifecycle(self) -> bool {
                match self {
                    $(Self::$variant => handler_kinds!(@role $role),)*
                }
            }

            /// The `Event` variant the handler receives.
            ///
            /// Event kinds follow the keyword convention: `message` is `Event::Message` and
            /// `MessageEvent`, `url` is `Event::Url` and `UrlEvent`, and so on. Only called for
            /// event kinds; the lifecycle hooks have no event variant.
            pub(super) fn event_variant(self) -> Ident {
                format_ident!("{}", pascal_case(self.name()))
            }

            /// The trait method's event parameter: its name and the event type at the
            /// `zeta_plugin` root. `None` for `loaded` and `shutdown`, which take no event.
            pub(super) fn event_parameter(self) -> Option<(String, String)> {
                if self.is_lifecycle() {
                    return None;
                }

                let keyword = self.name();

                Some((keyword.to_owned(), format!("{}Event", pascal_case(keyword))))
            }

            /// Whether the trait handler takes `&mut self`.
            ///
            /// Only the eventless lifecycle hooks do; every event handler takes `&self`.
            pub(super) const fn mutable_receiver(self) -> bool {
                self.is_lifecycle()
            }

            /// The `Subscriptions` method that registers this event kind, or `None` for the
            /// eventless lifecycle hooks.
            pub(super) fn subscription(self) -> Option<Ident> {
                (!self.is_lifecycle()).then(|| format_ident!("receive_{}", self.name()))
            }
        }
    };
    (@role Event) => {
        false
    };
    (@role Lifecycle) => {
        true
    };
}

handler_kinds! {
    Command("command", Event),
    Url("url", Event),
    Message("message", Event),
    Join("join", Event),
    Part("part", Event),
    Quit("quit", Event),
    Nick("nick", Event),
    Kick("kick", Event),
    Ctcp("ctcp", Event),
    Raw("raw", Event),
    Loaded("loaded", Lifecycle),
    Shutdown("shutdown", Lifecycle),
}
