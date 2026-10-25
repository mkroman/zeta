//! The `#[plugin]` attribute macro for the zeta IRC bot's plugin API.
//!
//! `#[plugin]` is applied to an **inherent impl block** and generates the
//! `zeta_plugin::Plugin` implementation for the block's type: the plugin's name and settings
//! type, the constructor that registers the declarative subscriptions, and the event handlers.
//! The impl block itself — its methods and their logic — is emitted unchanged beside it, with
//! only the macro's own annotations stripped.
//!
//! Besides the implementation, the macro submits a `zeta_plugin::PluginRegistration` through
//! `inventory`, so the host discovers every compiled-in plugin at startup without a central
//! list. Adding a plugin therefore needs only its module and cargo feature, not an entry
//! anywhere else.
//!
//! ```
//! use zeta_plugin::{Client, Context, Error, prelude::*};
//!
//! /// Greets people in the channel the command came from.
//! #[derive(Default)]
//! struct Greet;
//!
//! #[plugin(name = "greet")]
//! impl Greet {
//!     #[command(".hello", "Greet someone")]
//!     async fn hello(&self, _: &Context, client: &Client, channel: &str) -> Result<(), Error> {
//!         client.send_privmsg(channel, "hello!")?;
//!
//!         Ok(())
//!     }
//! }
//! ```
//!
//! # Container attributes
//!
//! | attribute | meaning |
//! |---|---|
//! | `name = "…"` | **Required.** The plugin's name (`Plugin::NAME`), which must match its module name. |
//! | `settings = Type` | The plugin's settings type; defaults to `NoSettings`. |
//! | `urls(any)` | Receives every URL posted in any channel. |
//! | `urls(hosts = EXPR)` | Receives the URLs whose host is in `EXPR` (a `&[&str]`). |
//! | `messages` | Receives every channel message. |
//!
//! The generated constructor registers the URLs, then the message subscription, then the
//! `#[command]`s (in declaration order), and finally delegates to the inherent constructor or
//! `Default::default()` when the block declares none.
//!
//! # Inherent constructor
//!
//! When the impl block contains an associated `fn new` whose parameters are drawn from
//! `&Context`, an immutable reference to the plugin's settings type, and `&mut Subscriptions` —
//! any subset of the three, in any order — the generated constructor registers the declarative
//! subscriptions and then calls it with those parameters in the declared order:
//!
//! ```
//! use zeta_plugin::prelude::*;
//!
//! struct Greet;
//!
//! #[plugin(name = "greet")]
//! impl Greet {
//!     fn new(_: &Context, subscriptions: &mut Subscriptions) -> Result<Self, Error> {
//!         subscriptions.receive_join();
//!
//!         Ok(Self)
//!     }
//! }
//! ```
//!
//! A `new` with no recognized parameters remains an ordinary inherent method, so zero-parameter
//! helpers keep working; mixing recognized and unrecognized parameter types is an error, because
//! it is almost always a typo. Otherwise the block's type must implement `Default` (unit structs:
//! `#[derive(Default)]`), and the constructor returns it.
//!
//! # Method attributes
//!
//! `#[command("trigger", "description", …)]` on an inherent async method serves a command. The
//! optional named arguments, in any order after the two strings, are:
//!
//! | option | meaning |
//! |---|---|
//! | `args = Opts` | Parses the arguments into `Opts` (an `argh::FromArgs + argh::ArgsInfo` type) through `parse_args_or_usage`. |
//! | `words` | With `args`, parses through `parse_words_or_usage` instead, preserving free-form text. |
//! | `spec = NAME` | Overrides the generated `CommandSpec` constant's name; the default folds the trigger to upper case (`.dig` → `DIG`). |
//! | `label = "Dig"` | The display label for generated usage and error replies; defaults to the plugin's name. |
//! | `event` | The handler also receives the `&CommandEvent`, and may omit the `channel` parameter. |
//! | `register_if = expr` | Registers the command only when `expr` evaluates to `true` (e.g. `settings.enable_command`); the expression runs in the generated constructor, where `settings` is in scope. |
//!
//! The method's signature convention is strict: `async fn m(&self, ctx: &Context, client:
//! &Client, channel: &str [, command: &CommandEvent] [, opts: Opts]) -> Result<(), Error>`,
//! where `channel` may be omitted when `event` is set (the event then provides it), `command`
//! appears only with `event`, and `opts` only with `args`.
//!
//! `#[handler(kind)]` on an inherent async method forwards one of the trait's event handlers to
//! it verbatim, where `kind` is one of `command`, `url`, `message`, `join`, `part`, `quit`,
//! `nick`, `kick`, `ctcp`, `raw`, `loaded`, or `shutdown`. Kinds the block does not annotate
//! keep the trait's default implementation.
//!
//! A `#[cfg]` or `#[cfg_attr]` on a `#[command]` or `#[handler]` method is copied to the
//! generated constant, registration, dispatch arm, and forwarding method, so feature-gated
//! methods can be declared in the same impl block.

use proc_macro::TokenStream;
use syn::parse_macro_input;

mod args;
mod expand;

/// Implements [`Plugin`](https://docs.rs/zeta-plugin) for an inherent impl block; see the
/// [crate documentation](self) for the attribute grammar.
#[proc_macro_attribute]
pub fn plugin(attributes: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attributes as args::PluginArgs);
    let inherent = parse_macro_input!(item as syn::ItemImpl);

    expand::expand(&args, inherent).into()
}

#[cfg(test)]
mod tests {
    use super::expand::spec_const_name;

    #[test]
    fn derives_constant_names_from_triggers() {
        let name = |trigger| spec_const_name(trigger, proc_macro2::Span::call_site()).unwrap();

        assert_eq!(name(".dig"), "DIG");
        assert_eq!(name(".health"), "HEALTH");
        assert_eq!(name("!imdb"), "IMDB");
        assert_eq!(name(".yt-dlp"), "YT_DLP");
        assert_eq!(name(".long__trigger__"), "LONG_TRIGGER");
    }

    #[test]
    fn digit_leading_triggers_get_an_underscore() {
        let name = |trigger| spec_const_name(trigger, proc_macro2::Span::call_site()).unwrap();

        assert_eq!(name(".1st"), "_1ST");
    }

    #[test]
    fn punctuation_only_triggers_are_rejected() {
        let error = spec_const_name(".!?!", proc_macro2::Span::call_site())
            .expect_err("no constant name can be derived");

        assert!(error.to_string().contains("spec = NAME"));
    }
}
