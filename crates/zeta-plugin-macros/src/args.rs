//! Attribute argument parsing for the [`plugin`](crate::plugin) macro.
//!
//! The attribute grammar is parsed by hand instead of through [`syn::Meta`]: the values of
//! `urls(hosts = …)` and `register_if = …` are arbitrary expressions, which the `Meta` grammar
//! cannot represent.

use syn::parse::{Parse, ParseStream};
use syn::{Expr, Ident, LitStr, Token, Type};

/// The options `#[plugin(...)]` accepts, for the unknown-option diagnostic.
const PLUGIN_OPTIONS: &[&str] = &["name", "settings", "urls", "messages"];

/// The options `#[command(...)]` accepts, for the unknown-option diagnostic.
const COMMAND_OPTIONS: &[&str] = &["args", "words", "spec", "label", "event", "register_if"];

/// Parses the `= value` following an option name.
fn parse_value<T: Parse>(input: ParseStream<'_>) -> syn::Result<T> {
    input.parse::<Token![=]>()?;
    input.parse()
}

/// Stores `value` in `slot`, rejecting a repeated option.
fn set_once<T>(slot: &mut Option<T>, value: T, ident: &Ident) -> syn::Result<()> {
    if slot.is_some() {
        return Err(syn::Error::new(
            ident.span(),
            format!("duplicate `{ident}` argument"),
        ));
    }

    *slot = Some(value);

    Ok(())
}

/// Sets a boolean flag, rejecting a repetition.
fn set_flag(slot: &mut bool, ident: &Ident) -> syn::Result<()> {
    if *slot {
        return Err(syn::Error::new(
            ident.span(),
            format!("duplicate `{ident}` flag"),
        ));
    }

    *slot = true;

    Ok(())
}

/// The error for an option the attribute does not accept.
fn unknown_option(ident: &Ident, noun: &str, expected: &[&str]) -> syn::Error {
    let expected = expected
        .iter()
        .map(|option| format!("`{option}`"))
        .collect::<Vec<_>>()
        .join(", ");

    syn::Error::new(
        ident.span(),
        format!("unknown {noun} `{ident}`; expected {expected}"),
    )
}

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
                    let value: LitStr = parse_value(input)?;
                    set_once(&mut name, value, &ident)?;
                }
                "settings" => {
                    let value: Type = parse_value(input)?;
                    set_once(&mut settings, value, &ident)?;
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
                    set_once(&mut urls, value, &ident)?;
                }
                "messages" => set_flag(&mut messages, &ident)?,
                _ => return Err(unknown_option(&ident, "plugin attribute", PLUGIN_OPTIONS)),
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
                    let value: Type = parse_value(input)?;
                    set_once(&mut this.args, value, &ident)?;
                }
                "words" => set_flag(&mut this.words, &ident)?,
                "spec" => {
                    let value: Ident = parse_value(input)?;
                    set_once(&mut this.spec, value, &ident)?;
                }
                "label" => {
                    let value: LitStr = parse_value(input)?;
                    set_once(&mut this.label, value, &ident)?;
                }
                "event" => set_flag(&mut this.event, &ident)?,
                "register_if" => {
                    let value: Expr = parse_value(input)?;
                    set_once(&mut this.register_if, value, &ident)?;
                }
                _ => return Err(unknown_option(&ident, "command option", COMMAND_OPTIONS)),
            }
        }

        Ok(this)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_plugin_argument() {
        let args: PluginArgs =
            syn::parse_str("name = \"dig\", settings = Settings, urls(hosts = HOSTS), messages")
                .expect("the arguments should parse");

        assert_eq!(args.name.value(), "dig");
        assert!(args.settings.is_some());
        assert!(matches!(args.urls, Some(UrlsArgs::Hosts(_))));
        assert!(args.messages);
    }

    #[test]
    fn parses_urls_any() {
        let args: PluginArgs =
            syn::parse_str("name = \"dig\", urls(any)").expect("the arguments should parse");

        assert!(matches!(args.urls, Some(UrlsArgs::Any)));
    }

    #[test]
    fn requires_the_plugin_name() {
        let error = syn::parse_str::<PluginArgs>("settings = Settings")
            .err()
            .expect("the name is required");

        assert!(error.to_string().contains("name"), "{error}");
    }

    #[test]
    fn rejects_duplicate_plugin_arguments() {
        for arguments in [
            "name = \"a\", name = \"b\"",
            "name = \"a\", settings = A, settings = B",
            "name = \"a\", urls(any), urls(any)",
            "name = \"a\", messages, messages",
        ] {
            let error = syn::parse_str::<PluginArgs>(arguments)
                .err()
                .unwrap_or_else(|| panic!("`{arguments}` should be rejected"));

            assert!(error.to_string().contains("duplicate"), "{error}");
        }
    }

    #[test]
    fn rejects_unknown_plugin_arguments() {
        let error = syn::parse_str::<PluginArgs>("name = \"a\", commands(.x)")
            .err()
            .expect("an unknown argument should be rejected");

        assert!(
            error.to_string().contains("unknown plugin attribute"),
            "{error}"
        );
    }

    #[test]
    fn parses_every_command_option() {
        let args: CommandArgs = syn::parse_str(
            "\".test\", \"A test\", args = Opts, words, spec = TEST, label = \"Test\", event, \
             register_if = settings.enabled",
        )
        .expect("the arguments should parse");

        assert_eq!(args.trigger.value(), ".test");
        assert!(args.args.is_some());
        assert!(args.words);
        assert_eq!(args.spec.expect("spec should be set").to_string(), "TEST");
        assert_eq!(args.label.expect("label should be set").value(), "Test");
        assert!(args.event);
        assert!(args.register_if.is_some());
    }

    #[test]
    fn parses_a_command_without_options() {
        let args: CommandArgs =
            syn::parse_str("\".test\", \"A test\"").expect("the arguments should parse");

        assert!(args.args.is_none());
        assert!(!args.words);
        assert!(!args.event);
    }

    #[test]
    fn rejects_duplicate_command_options() {
        for arguments in [
            "\".test\", \"A test\", args = A, args = B",
            "\".test\", \"A test\", words, words",
            "\".test\", \"A test\", spec = A, spec = B",
            "\".test\", \"A test\", label = \"a\", label = \"b\"",
            "\".test\", \"A test\", event, event",
            "\".test\", \"A test\", register_if = a, register_if = b",
        ] {
            let error = syn::parse_str::<CommandArgs>(arguments)
                .err()
                .unwrap_or_else(|| panic!("`{arguments}` should be rejected"));

            assert!(error.to_string().contains("duplicate"), "{error}");
        }
    }

    #[test]
    fn rejects_unknown_command_options() {
        let error = syn::parse_str::<CommandArgs>("\".test\", \"A test\", description = \"x\"")
            .err()
            .expect("an unknown option should be rejected");

        assert!(
            error.to_string().contains("unknown command option"),
            "{error}"
        );
    }
}
