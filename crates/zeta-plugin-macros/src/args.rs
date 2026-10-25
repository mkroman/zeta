//! Attribute argument parsing for the [`plugin`](crate::plugin) macro.
//!
//! The attribute grammar is parsed by hand instead of through [`syn::Meta`]: the values of
//! `urls(hosts = …)` and `register_if = …` are arbitrary expressions, which the `Meta` grammar
//! cannot represent.

use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::{Expr, Ident, LitStr, Token, Type};

/// The arguments of the `#[plugin(...)]` container attribute.
pub struct PluginArgs {
    /// The required plugin name, e.g. `"dig"`.
    pub name: LitStr,
    /// The plugin's settings type; `NoSettings` when absent.
    pub settings: Option<Type>,
    /// The URL registration, from `urls(any)` or `urls(hosts = EXPR)`.
    pub urls: Option<UrlsArgs>,
    /// Whether every channel message is received, from `messages`.
    pub messages: bool,
}

/// The `urls(...)` container argument.
#[derive(Clone)]
pub enum UrlsArgs {
    /// Every URL posted in any channel.
    Any,
    /// The URLs whose host is in the given expression (`&[&str]`).
    Hosts(Expr),
}

impl Parse for PluginArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut name: Option<LitStr> = None;
        let mut settings: Option<Type> = None;
        let mut urls: Option<UrlsArgs> = None;
        let mut messages = false;

        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            match ident.to_string().as_str() {
                "name" => {
                    input.parse::<Token![=]>()?;
                    let value: LitStr = input.parse()?;
                    if name.is_some() {
                        return Err(syn::Error::new(value.span(), "duplicate `name` argument"));
                    }
                    name = Some(value);
                }
                "settings" => {
                    input.parse::<Token![=]>()?;
                    let value: Type = input.parse()?;
                    if settings.is_some() {
                        return Err(syn::Error::new(
                            value.span(),
                            "duplicate `settings` argument",
                        ));
                    }
                    settings = Some(value);
                }
                "urls" => {
                    let content;
                    syn::parenthesized!(content in input);
                    let kind: Ident = content.parse()?;
                    let value = match kind.to_string().as_str() {
                        "any" => UrlsArgs::Any,
                        "hosts" => {
                            content.parse::<Token![=]>()?;
                            UrlsArgs::Hosts(content.parse()?)
                        }
                        _ => {
                            return Err(syn::Error::new(
                                kind.span(),
                                "expected `urls(any)` or `urls(hosts = EXPR)`",
                            ));
                        }
                    };
                    if urls.is_some() {
                        return Err(syn::Error::new(kind.span(), "duplicate `urls` argument"));
                    }
                    urls = Some(value);
                }
                "messages" => messages = true,
                other => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!(
                            "unknown plugin attribute `{other}`; expected `name`, `settings`, \
                             `urls`, or `messages`"
                        ),
                    ));
                }
            }

            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(Self {
            name: name.ok_or_else(|| {
                syn::Error::new(
                    proc_macro2::Span::call_site(),
                    "missing required `name = \"…\"` argument",
                )
            })?,
            settings,
            urls,
            messages,
        })
    }
}

/// The arguments of a `#[command(...)]` method attribute.
pub struct CommandArgs {
    /// The command trigger as users type it, e.g. `.dig`.
    pub trigger: LitStr,
    /// The user-facing description shown by the host's help command.
    pub description: Expr,
    /// The `argh` options type, from `args = Opts`.
    pub args: Option<Type>,
    /// Whether the arguments are parsed with `parse_words_or_usage`, from `words`.
    ///
    /// Requires `args`.
    pub words: bool,
    /// The explicit `CommandSpec` constant name, from `spec = NAME`.
    pub spec: Option<Ident>,
    /// The display label for generated usage replies, from `label = "Dig"`.
    pub label: Option<LitStr>,
    /// Whether the handler also receives the `CommandEvent`, from `event`.
    pub event: bool,
    /// The registration condition, from `register_if = expr`.
    pub register_if: Option<Expr>,
}

impl Parse for CommandArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let trigger: LitStr = input.parse()?;
        input.parse::<Token![,]>()?;
        let description: Expr = input.parse()?;

        let mut this = Self {
            trigger,
            description,
            args: None,
            words: false,
            spec: None,
            label: None,
            event: false,
            register_if: None,
        };

        while input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }

            let ident: Ident = input.parse()?;
            match ident.to_string().as_str() {
                "args" => {
                    input.parse::<Token![=]>()?;
                    let value: Type = input.parse()?;
                    if this.args.is_some() {
                        return Err(syn::Error::new(value.span(), "duplicate `args` option"));
                    }
                    this.args = Some(value);
                }
                "words" => this.words = true,
                "spec" => {
                    input.parse::<Token![=]>()?;
                    let value: Ident = input.parse()?;
                    if this.spec.is_some() {
                        return Err(syn::Error::new(value.span(), "duplicate `spec` option"));
                    }
                    this.spec = Some(value);
                }
                "label" => {
                    input.parse::<Token![=]>()?;
                    let value: LitStr = input.parse()?;
                    if this.label.is_some() {
                        return Err(syn::Error::new(value.span(), "duplicate `label` option"));
                    }
                    this.label = Some(value);
                }
                "event" => this.event = true,
                "register_if" => {
                    input.parse::<Token![=]>()?;
                    let value: Expr = input.parse()?;
                    if this.register_if.is_some() {
                        return Err(syn::Error::new(
                            value.span(),
                            "duplicate `register_if` option",
                        ));
                    }
                    this.register_if = Some(value);
                }
                other => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!(
                            "unknown command option `{other}`; expected `args`, `words`, `spec`, \
                             `label`, `event`, or `register_if`"
                        ),
                    ));
                }
            }
        }

        Ok(this)
    }
}
