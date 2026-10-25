//! Classification, validation, and code generation for the [`plugin`](crate::plugin) macro.
//!
//! The macro is applied to an inherent impl block. Classification reads the `#[command]` and
//! `#[handler]` annotations off the block's methods and validates them, rendering then emits
//! the `Plugin` implementation next to the stripped inherent impl. Every generated path is
//! absolute (`::zeta_plugin::…`), because a proc macro has no `$crate`.

use proc_macro2::{Span, TokenStream};
use quote::{ToTokens, format_ident, quote};
use syn::parse::Parse;
use syn::spanned::Spanned;
use syn::{GenericArgument, Ident, ImplItem, ImplItemFn, PathArguments, Type};

use crate::args::{CommandArgs, PluginArgs, UrlsArgs};

/// The classified contents of a `#[plugin]` impl block.
pub struct Plugin {
    /// The plugin's name, from the required `name = "…"` argument.
    name: syn::LitStr,
    /// The plugin's settings type, or `None` for `NoSettings`.
    settings: Option<Type>,
    /// The URL registration, if any.
    urls: Option<UrlsArgs>,
    /// Whether every channel message is received.
    messages: bool,
    /// The inherent constructor, when the impl block declared `fn new`.
    constructor: bool,
    /// The `#[command]` methods, in declaration order.
    commands: Vec<Command>,
    /// The `#[handler(kind)]` methods, in declaration order.
    handlers: Vec<Handler>,
}

/// One `#[command]` method.
struct Command {
    /// The module-level `CommandSpec` constant the command is registered with.
    spec: Ident,
    /// The parsed attribute arguments.
    args: CommandArgs,
    /// The inherent method implementing the command.
    method: Ident,
    /// The method's `#[cfg]`/`#[cfg_attr]` attributes, copied to everything generated for it.
    cfg: Vec<syn::Attribute>,
}

/// One `#[handler(kind)]` method.
struct Handler {
    /// The event kind the method handles.
    kind: HandlerKind,
    /// The inherent method handling the event.
    method: Ident,
    /// The span of the `#[handler]` attribute, for conflict errors.
    span: Span,
    /// The method's `#[cfg]`/`#[cfg_attr]` attributes, copied to the forwarding method.
    cfg: Vec<syn::Attribute>,
}

/// The event kinds a `#[handler(kind)]` method can handle.
#[derive(Clone, Copy, PartialEq)]
enum HandlerKind {
    Command,
    Url,
    Message,
    Join,
    Part,
    Quit,
    Nick,
    Kick,
    Ctcp,
    Raw,
    Loaded,
    Shutdown,
}

impl HandlerKind {
    /// Parses the `kind` argument of `#[handler(kind)]`.
    fn from_ident(ident: &Ident) -> Option<Self> {
        Some(match ident.to_string().as_str() {
            "command" => Self::Command,
            "url" => Self::Url,
            "message" => Self::Message,
            "join" => Self::Join,
            "part" => Self::Part,
            "quit" => Self::Quit,
            "nick" => Self::Nick,
            "kick" => Self::Kick,
            "ctcp" => Self::Ctcp,
            "raw" => Self::Raw,
            "loaded" => Self::Loaded,
            "shutdown" => Self::Shutdown,
            _ => return None,
        })
    }

    /// The kind as written in `#[handler(kind)]`, for diagnostics.
    const fn name(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::Url => "url",
            Self::Message => "message",
            Self::Join => "join",
            Self::Part => "part",
            Self::Quit => "quit",
            Self::Nick => "nick",
            Self::Kick => "kick",
            Self::Ctcp => "ctcp",
            Self::Raw => "raw",
            Self::Loaded => "loaded",
            Self::Shutdown => "shutdown",
        }
    }

    /// The trait method the handler is forwarded through.
    const fn trait_method(self) -> &'static str {
        match self {
            Self::Command => "handle_command",
            Self::Url => "handle_url",
            Self::Message => "handle_message",
            Self::Join => "handle_join",
            Self::Part => "handle_part",
            Self::Quit => "handle_quit",
            Self::Nick => "handle_nick",
            Self::Kick => "handle_kick",
            Self::Ctcp => "handle_ctcp",
            Self::Raw => "handle_raw",
            Self::Loaded => "loaded",
            Self::Shutdown => "shutdown",
        }
    }

    /// The trait method's event parameter: its name and the event type at the `zeta_plugin`
    /// root. `None` for `loaded` and `shutdown`, which take no event.
    const fn event_parameter(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Command => Some(("command", "CommandEvent")),
            Self::Url => Some(("url", "UrlEvent")),
            Self::Message => Some(("message", "MessageEvent")),
            Self::Join => Some(("join", "JoinEvent")),
            Self::Part => Some(("part", "PartEvent")),
            Self::Quit => Some(("quit", "QuitEvent")),
            Self::Nick => Some(("nick", "NickEvent")),
            Self::Kick => Some(("kick", "KickEvent")),
            Self::Ctcp => Some(("ctcp", "CtcpEvent")),
            Self::Raw => Some(("raw", "RawEvent")),
            Self::Loaded | Self::Shutdown => None,
        }
    }

    /// Whether the trait handler takes `&mut self`.
    const fn mutable_receiver(self) -> bool {
        matches!(self, Self::Loaded | Self::Shutdown)
    }
}

/// Classifies, validates, and renders the macro output for one `#[plugin]` impl block.
pub fn expand(args: &PluginArgs, mut inherent: syn::ItemImpl) -> TokenStream {
    match classify(args, &inherent) {
        Ok(plugin) => {
            strip_annotations(&plugin, &mut inherent);

            render(&plugin, &inherent)
        }
        Err(error) => error.to_compile_error(),
    }
}

/// Rejects trait impls and generic impl blocks, which the macro cannot implement `Plugin` for.
fn reject_target(inherent: &syn::ItemImpl) -> Option<syn::Error> {
    if let Some((_, trait_path, _)) = &inherent.trait_ {
        return Some(syn::Error::new(
            trait_path.span(),
            "`#[plugin]` must be applied to an inherent impl block, not a trait implementation",
        ));
    }

    if !inherent.generics.params.is_empty() {
        return Some(syn::Error::new(
            inherent.generics.span(),
            "`#[plugin]` does not support generic impl blocks",
        ));
    }

    None
}

/// Reads the `#[command]` and `#[handler]` annotations and validates every rule the macro
/// enforces, returning the first error found with its original span.
fn classify(args: &PluginArgs, inherent: &syn::ItemImpl) -> syn::Result<Plugin> {
    if let Some(error) = reject_target(inherent) {
        return Err(error);
    }

    let mut commands: Vec<Command> = Vec::new();
    let mut handlers: Vec<Handler> = Vec::new();
    let mut constructor = false;

    for item in &inherent.items {
        let ImplItem::Fn(method) = item else {
            continue;
        };

        let mut command: Option<(CommandArgs, Span)> = None;
        let mut handler: Option<(HandlerKind, Span)> = None;

        for attribute in &method.attrs {
            if attribute.path().is_ident("command") {
                if let Some((_, _)) = &command {
                    return Err(syn::Error::new(
                        attribute.span(),
                        "duplicate `#[command]` attribute",
                    ));
                }

                command = Some((
                    attribute.parse_args_with(<CommandArgs as Parse>::parse)?,
                    attribute.span(),
                ));
            } else if attribute.path().is_ident("handler") {
                if handler.is_some() {
                    return Err(syn::Error::new(
                        attribute.span(),
                        "duplicate `#[handler]` attribute",
                    ));
                }

                let kind: Ident = attribute
                    .parse_args_with(|input: syn::parse::ParseStream<'_>| input.parse::<Ident>())?;
                let kind = HandlerKind::from_ident(&kind).ok_or_else(|| {
                    syn::Error::new(
                        kind.span(),
                        "unknown event kind; expected one of `command`, `url`, `message`, `join`, \
                         `part`, `quit`, `nick`, `kick`, `ctcp`, `raw`, `loaded`, `shutdown`",
                    )
                })?;

                handler = Some((kind, attribute.span()));
            }
        }

        match command {
            Some((args, span)) => {
                if handler.is_some() {
                    return Err(syn::Error::new(
                        span,
                        "a method cannot be both `#[command]` and `#[handler]`; a `#[command]` \
                         method is dispatched through the generated `handle_command`",
                    ));
                }

                validate_command(&args, method)?;

                let spec = match &args.spec {
                    Some(spec) => spec.clone(),
                    None => spec_const_name(&args.trigger.value(), args.trigger.span())?,
                };

                commands.push(Command {
                    spec,
                    args,
                    method: method.sig.ident.clone(),
                    cfg: cfg_attributes(method),
                });
            }
            None => match handler {
                Some((kind, span)) => {
                    classify_handler(&mut handlers, kind, span, method)?;
                }
                None => classify_constructor(method, &mut constructor)?,
            },
        }
    }

    validate_duplicates(&commands)?;
    validate_conflicts(&commands, &handlers)?;

    Ok(Plugin {
        name: args.name.clone(),
        settings: args.settings.clone(),
        urls: args.urls.clone(),
        messages: args.messages,
        constructor,
        commands,
        handlers,
    })
}

/// Validates one `#[handler(kind)]` method and appends it to `handlers`.
fn classify_handler(
    handlers: &mut Vec<Handler>,
    kind: HandlerKind,
    span: Span,
    method: &ImplItemFn,
) -> syn::Result<()> {
    validate_handler(kind, method)?;

    if let Some(existing) = handlers.iter().find(|handler| handler.kind == kind) {
        return Err(syn::Error::new(
            span,
            format!(
                "duplicate `#[handler({})]` method; `{}` already handles that event kind",
                kind.name(),
                existing.method,
            ),
        ));
    }

    handlers.push(Handler {
        kind,
        method: method.sig.ident.clone(),
        span,
        cfg: cfg_attributes(method),
    });

    Ok(())
}

/// Validates the inherent constructor when `method` is the three-parameter `fn new`.
fn classify_constructor(method: &ImplItemFn, constructor: &mut bool) -> syn::Result<()> {
    if !is_constructor(method) {
        return Ok(());
    }

    if !cfg_attributes(method).is_empty() {
        return Err(syn::Error::new(
            method.sig.ident.span(),
            "the plugin constructor cannot be gated with `#[cfg]`; the generated `Plugin::new` \
             always calls it",
        ));
    }

    validate_constructor(method)?;
    *constructor = true;

    Ok(())
}

/// The method's `#[cfg]`/`#[cfg_attr]` attributes, in declaration order.
fn cfg_attributes(method: &ImplItemFn) -> Vec<syn::Attribute> {
    method
        .attrs
        .iter()
        .filter(|attribute| {
            attribute.path().is_ident("cfg") || attribute.path().is_ident("cfg_attr")
        })
        .cloned()
        .collect()
}

/// Whether the method is the inherent plugin constructor.
///
/// Only an associated `fn new` with exactly three typed parameters is recognized. Other `new`
/// methods are ordinary inherent methods, so helper constructors (e.g. `fn new() -> Self`) can
/// live beside the plugin impl.
fn is_constructor(method: &ImplItemFn) -> bool {
    method.sig.ident == "new"
        && !method
            .sig
            .inputs
            .iter()
            .any(|input| matches!(input, syn::FnArg::Receiver(_)))
        && typed_parameters(method).len() == 3
}

/// Checks that no two commands share a trigger or a `CommandSpec` constant name.
fn validate_duplicates(commands: &[Command]) -> syn::Result<()> {
    for (index, command) in commands.iter().enumerate() {
        for previous in &commands[..index] {
            if previous.args.trigger.value() == command.args.trigger.value() {
                return Err(syn::Error::new(
                    command.args.trigger.span(),
                    format!(
                        "duplicate command trigger `{}`",
                        command.args.trigger.value()
                    ),
                ));
            }

            if previous.spec == command.spec {
                return Err(syn::Error::new(
                    command.spec.span(),
                    format!("duplicate command constant name `{}`", command.spec),
                ));
            }
        }
    }

    Ok(())
}

/// Rejects a plugin that combines `#[command]` methods with `#[handler(command)]`.
fn validate_conflicts(commands: &[Command], handlers: &[Handler]) -> syn::Result<()> {
    if commands.is_empty() {
        return Ok(());
    }

    if let Some(handler) = handlers
        .iter()
        .find(|handler| handler.kind == HandlerKind::Command)
    {
        return Err(syn::Error::new(
            handler.span,
            "a plugin cannot combine `#[command]` methods with `#[handler(command)]`; the \
             generated dispatch and the forwarding handler would both implement \
             `handle_command`",
        ));
    }

    Ok(())
}

/// Derives the `CommandSpec` constant name from a trigger: uppercased, with every
/// non-alphanumeric character folded to `_`, runs of underscores collapsed, and leading and
/// trailing underscores trimmed (`.dig` → `DIG`, `.yt-dlp` → `YT_DLP`).
pub fn spec_const_name(trigger: &str, span: Span) -> syn::Result<Ident> {
    let folded: String = trigger
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();

    let name = folded.trim_matches('_');
    if name.is_empty() {
        return Err(syn::Error::new(
            span,
            format!(
                "cannot derive a constant name from the trigger `{trigger}`; name one explicitly \
                 with `spec = NAME`"
            ),
        ));
    }

    // Collapse runs of underscores, so `.yt--dlp` names `YT_DLP`.
    let mut collapsed = String::with_capacity(name.len());
    for character in name.chars() {
        if character == '_' && collapsed.ends_with('_') {
            continue;
        }
        collapsed.push(character);
    }

    // A trigger may fold to something a constant cannot be named, e.g. `.1st`.
    let name = if collapsed.starts_with(|character: char| character.is_ascii_digit()) {
        format!("_{collapsed}")
    } else {
        collapsed
    };

    Ok(Ident::new(&name, span))
}

/// Validates the strict signature convention for `#[command]` methods:
/// `async fn m(&self, ctx: &Context, client: &Client, channel: &str [, command: &CommandEvent]
/// [, opts: Opts]) -> Result<(), Error>`.
fn validate_command(args: &CommandArgs, method: &ImplItemFn) -> syn::Result<()> {
    let signature = &method.sig;
    let name = &signature.ident;

    if signature.asyncness.is_none() {
        return Err(syn::Error::new(
            name.span(),
            "command handlers must be `async`",
        ));
    }

    if args.words && args.args.is_none() {
        return Err(syn::Error::new(
            args.trigger.span(),
            "`words` requires `args = Opts`: it selects how the options are parsed",
        ));
    }

    expect_receiver(method, false)?;
    let typed = typed_parameters(method);

    let mut expected: Vec<&str> = vec!["Context", "Client", "str"];
    if args.event {
        expected.push("CommandEvent");
    }

    let options_index = expected.len();
    let required = options_index + usize::from(args.args.is_some());
    if typed.len() < required {
        let with_event = if args.event {
            ", command: &CommandEvent"
        } else {
            ""
        };
        let with_opts = if args.args.is_some() { ", opts" } else { "" };

        return Err(syn::Error::new(
            name.span(),
            format!(
                "command handlers must take `(&self, ctx: &Context, client: &Client, \
                 channel: &str{with_event}{with_opts})`"
            ),
        ));
    }

    for (index, expected_type) in expected.iter().enumerate() {
        expect_borrowed(typed[index].ty.as_ref(), expected_type, false)?;
    }

    if let Some(options) = &args.args {
        let actual = typed[options_index].ty.as_ref();
        if matches!(actual, Type::Reference(_)) {
            return Err(syn::Error::new(
                actual.span(),
                format!(
                    "command options are taken by value: `{}`",
                    display_type(options)
                ),
            ));
        }

        if last_path_ident(options) != last_path_ident(actual) {
            return Err(syn::Error::new(
                actual.span(),
                format!(
                    "the command options parameter must have type `{}`",
                    display_type(options)
                ),
            ));
        }
    }

    if let Some(extra) = typed.get(required) {
        return Err(syn::Error::new(
            extra.ty.span(),
            "unexpected parameter in a command handler",
        ));
    }

    expect_result(signature, ExpectedFirst::Unit, "command handlers")
}

/// Validates the strict signature convention for `#[handler(kind)]` methods — the trait
/// handler's signature: `async fn m(&self | &mut self, ctx: &Context, client: &Client [, event])
/// -> Result<(), Error>`.
fn validate_handler(kind: HandlerKind, method: &ImplItemFn) -> syn::Result<()> {
    let signature = &method.sig;
    let name = &signature.ident;

    if signature.asyncness.is_none() {
        return Err(syn::Error::new(
            name.span(),
            format!("`#[handler({})]` methods must be `async`", kind.name()),
        ));
    }

    expect_receiver(method, kind.mutable_receiver())?;
    let typed = typed_parameters(method);

    let mut expected: Vec<&str> = vec!["Context", "Client"];
    if let Some((_, event)) = kind.event_parameter() {
        expected.push(event);
    }

    if typed.len() != expected.len() {
        let with_event = kind
            .event_parameter()
            .map_or_else(String::new, |(parameter, event)| {
                format!(", {parameter}: &{event}")
            });
        let receiver = if kind.mutable_receiver() {
            "&mut self"
        } else {
            "&self"
        };

        return Err(syn::Error::new(
            name.span(),
            format!(
                "`#[handler({})]` methods must take `({receiver}, ctx: &Context, client: \
                 &Client{with_event})`",
                kind.name()
            ),
        ));
    }

    for (index, expected_type) in expected.iter().enumerate() {
        expect_borrowed(typed[index].ty.as_ref(), expected_type, false)?;
    }

    expect_result(signature, ExpectedFirst::Unit, "handlers")
}

/// Validates the exact trait signature of the inherent constructor:
/// `fn new(ctx: &Context, settings: &Settings, subscriptions: &mut Subscriptions) ->
/// Result<Self, Error>`.
fn validate_constructor(method: &ImplItemFn) -> syn::Result<()> {
    let signature = &method.sig;
    let name = &signature.ident;

    if signature.asyncness.is_some() {
        return Err(syn::Error::new(
            name.span(),
            "the plugin constructor must not be `async`",
        ));
    }

    if signature
        .inputs
        .first()
        .is_some_and(|input| matches!(input, syn::FnArg::Receiver(_)))
    {
        return Err(syn::Error::new(
            name.span(),
            "the plugin constructor is an associated function and must not take `self`",
        ));
    }

    let typed = typed_parameters(method);
    if typed.len() != 3 {
        return Err(syn::Error::new(
            name.span(),
            "the plugin constructor must take `(ctx: &Context, settings: &Settings, \
             subscriptions: &mut Subscriptions)`",
        ));
    }

    expect_borrowed(typed[0].ty.as_ref(), "Context", false)?;
    // The settings type is named by the plugin author; only its borrowing is checked.
    if !matches!(typed[1].ty.as_ref(), Type::Reference(reference) if reference.mutability.is_none())
    {
        return Err(syn::Error::new(
            typed[1].ty.span(),
            "expected an immutable reference to the plugin's settings",
        ));
    }
    expect_borrowed(typed[2].ty.as_ref(), "Subscriptions", true)?;

    expect_result(signature, ExpectedFirst::Self_, "the plugin constructor")
}

/// The type the `Result`'s first generic argument must be.
#[derive(Clone, Copy)]
enum ExpectedFirst {
    /// `Result<(), Error>`, the handler return type.
    Unit,
    /// `Result<Self, Error>`, the constructor return type.
    Self_,
}

/// Checks that a signature returns `Result<first, _>`.
fn expect_result(
    signature: &syn::Signature,
    first: ExpectedFirst,
    subject: &str,
) -> syn::Result<()> {
    let expected = match first {
        ExpectedFirst::Unit => "()",
        ExpectedFirst::Self_ => "Self",
    };

    let syn::ReturnType::Type(_, output) = &signature.output else {
        return Err(syn::Error::new(
            signature.ident.span(),
            format!("{subject} must return `Result<{expected}, Error>`"),
        ));
    };

    let Type::Path(path) = output.as_ref() else {
        return Err(unexpected_return_type(output.span(), subject, expected));
    };
    let Some(segment) = path.path.segments.last() else {
        return Err(unexpected_return_type(output.span(), subject, expected));
    };
    if segment.ident != "Result" {
        return Err(unexpected_return_type(output.span(), subject, expected));
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Err(unexpected_return_type(output.span(), subject, expected));
    };

    let mut arguments = arguments.args.iter();
    match (arguments.next(), arguments.next(), arguments.next()) {
        (Some(first_argument), Some(_), None) => {
            let matches = match first {
                ExpectedFirst::Unit => is_unit(first_argument),
                ExpectedFirst::Self_ => is_self(first_argument),
            };
            if matches {
                Ok(())
            } else {
                Err(syn::Error::new(
                    first_argument.span(),
                    format!("{subject} must return `Result<{expected}, Error>`"),
                ))
            }
        }
        _ => Err(unexpected_return_type(output.span(), subject, expected)),
    }
}

/// The "must return `Result<…>`" error for an unexpected return type.
fn unexpected_return_type(span: Span, subject: &str, expected: &str) -> syn::Error {
    syn::Error::new(
        span,
        format!("{subject} must return `Result<{expected}, Error>`"),
    )
}

/// Whether the generic argument is the unit type.
fn is_unit(argument: &GenericArgument) -> bool {
    matches!(argument, GenericArgument::Type(Type::Tuple(tuple)) if tuple.elems.is_empty())
}

/// Whether the generic argument is `Self`.
fn is_self(argument: &GenericArgument) -> bool {
    matches!(argument, GenericArgument::Type(Type::Path(path))
        if path.path.segments.len() == 1 && path.path.segments[0].ident == "Self")
}

/// Checks that the method's receiver is `&self` — or `&mut self`, when `mutable`.
fn expect_receiver(method: &ImplItemFn, mutable: bool) -> syn::Result<()> {
    match method.sig.inputs.first() {
        Some(syn::FnArg::Receiver(receiver)) => {
            if receiver.reference.is_some() && receiver.mutability.is_some() == mutable {
                Ok(())
            } else {
                let expected = if mutable { "`&mut self`" } else { "`&self`" };

                Err(syn::Error::new(
                    receiver.span(),
                    format!("the method must take {expected}"),
                ))
            }
        }
        _ => Err(syn::Error::new(
            method.sig.ident.span(),
            "the method must take `self` by reference",
        )),
    }
}

/// The method's typed parameters, skipping the receiver.
fn typed_parameters(method: &ImplItemFn) -> Vec<&syn::PatType> {
    method
        .sig
        .inputs
        .iter()
        .filter_map(|input| match input {
            syn::FnArg::Typed(typed) => Some(typed),
            syn::FnArg::Receiver(_) => None,
        })
        .collect()
}

/// Checks that `ty` is a reference to a type whose last path segment is `expected`.
fn expect_borrowed(ty: &Type, expected: &str, mutable: bool) -> syn::Result<()> {
    let Type::Reference(reference) = ty else {
        return Err(syn::Error::new(
            ty.span(),
            format!("expected `&{expected}`"),
        ));
    };

    if reference.mutability.is_some() != mutable {
        let expected_reference = if mutable {
            format!("`&mut {expected}`")
        } else {
            format!("`&{expected}`")
        };

        return Err(syn::Error::new(
            reference.span(),
            format!("expected {expected_reference}"),
        ));
    }

    match last_path_ident(&reference.elem) {
        Some(ident) if ident == expected => Ok(()),
        _ => Err(syn::Error::new(
            reference.elem.span(),
            format!("expected `&{expected}`"),
        )),
    }
}

/// The last path segment of a type, looking through references.
fn last_path_ident(ty: &Type) -> Option<&Ident> {
    match ty {
        Type::Reference(reference) => last_path_ident(&reference.elem),
        Type::Path(path) => path.path.segments.last().map(|segment| &segment.ident),
        _ => None,
    }
}

/// Renders a type for an error message.
fn display_type(ty: &Type) -> String {
    ty.to_token_stream().to_string()
}

/// Removes the macro's own annotations from the inherent impl's methods and allows the lints
/// the macro's plumbing provokes on the user's code:
///
/// - annotated command and handler methods may never await (`unused_async`), because the
///   generated dispatch awaits them;
/// - `loaded` and `shutdown` handlers take `&mut self` because the trait's signature says so,
///   even when the body only reads (`needless_pass_by_ref_mut`);
/// - the recognized inherent constructor returns `Result` even when initialization cannot
///   fail (`unnecessary_wraps`), because the `Plugin` trait fixes its signature, and takes
///   `&NoSettings` by reference (`trivially_copy_pass_by_ref`) when the plugin has no
///   settings.
///
/// Plugins therefore carry no `#[allow]`s of their own for either.
fn strip_annotations(plugin: &Plugin, inherent: &mut syn::ItemImpl) {
    for item in &mut inherent.items {
        if let ImplItem::Fn(method) = item {
            let annotated = method.attrs.iter().any(|attribute| {
                attribute.path().is_ident("command") || attribute.path().is_ident("handler")
            });
            method.attrs.retain(|attribute| {
                !attribute.path().is_ident("command") && !attribute.path().is_ident("handler")
            });

            if annotated {
                let mutable = plugin.handlers.iter().any(|handler| {
                    handler.kind.mutable_receiver() && handler.method == method.sig.ident
                });

                if mutable {
                    method.attrs.push(syn::parse_quote!(
                        #[allow(
                            clippy::unused_async,
                            clippy::unused_async_trait_impl,
                            clippy::needless_pass_by_ref_mut
                        )
                    ]));
                } else {
                    method.attrs.push(syn::parse_quote!(
                        #[allow(clippy::unused_async, clippy::unused_async_trait_impl)]
                    ));
                }
            } else if plugin.constructor && method.sig.ident == "new" {
                method.attrs.push(syn::parse_quote!(
                    #[allow(clippy::unnecessary_wraps, clippy::trivially_copy_pass_by_ref)]
                ));
            }
        }
    }
}

/// Renders the macro output: the `CommandSpec` constants, the `Plugin` implementation, and the
/// stripped inherent impl.
fn render(plugin: &Plugin, inherent: &syn::ItemImpl) -> TokenStream {
    let self_ty = &inherent.self_ty;
    let name = &plugin.name;
    let settings = plugin.settings.as_ref().map_or_else(
        || quote!(::zeta_plugin::NoSettings),
        |settings| quote!(#settings),
    );

    let constants = plugin.commands.iter().map(|command| {
        let spec = &command.spec;
        let cfg = &command.cfg;
        let trigger = &command.args.trigger;
        let description = &command.args.description;
        let constructor = command.args.args.as_ref().map_or_else(
            || quote!(::zeta_plugin::CommandSpec::new(#trigger, #description)),
            |options| {
                quote!(::zeta_plugin::CommandSpec::with_args::<#options>(#trigger, #description))
            },
        );

        quote! {
            #(#cfg)*
            const #spec: ::zeta_plugin::CommandSpec = #constructor;
        }
    });

    let registrations = registrations(plugin);
    let constructor_tail = if plugin.constructor {
        quote!(Self::new(ctx, settings, subscriptions))
    } else {
        quote!(::core::result::Result::Ok(
            <Self as ::core::default::Default>::default()
        ))
    };
    let (context_parameter, settings_parameter, subscriptions_parameter) =
        constructor_parameters(plugin, !registrations.is_empty());

    let handle_command = (!plugin.commands.is_empty()).then(|| {
        let arms = plugin
            .commands
            .iter()
            .map(|command| dispatch_arm(command, &plugin.name));

        quote! {
            #[allow(unused_variables)]
            async fn handle_command(
                &self,
                ctx: &::zeta_plugin::Context,
                client: &::zeta_plugin::Client,
                command: &::zeta_plugin::CommandEvent,
            ) -> ::core::result::Result<(), ::zeta_plugin::Error> {
                match command.spec.trigger() {
                    #(#arms)*
                    _ => {}
                }

                ::core::result::Result::Ok(())
            }
        }
    });

    let handlers = plugin.handlers.iter().map(handler_impl);

    quote! {
        #(#constants)*

        #[::zeta_plugin::async_trait::async_trait]
        impl ::zeta_plugin::Plugin<::zeta_plugin::Context> for #self_ty {
            const NAME: &'static str = #name;
            type Settings = #settings;

            #[allow(unused_variables)]
            fn new(
                #context_parameter: &::zeta_plugin::Context,
                #settings_parameter: &#settings,
                #subscriptions_parameter: &mut ::zeta_plugin::Subscriptions,
            ) -> ::core::result::Result<Self, ::zeta_plugin::Error> {
                #(#registrations)*

                #constructor_tail
            }

            #handle_command

            #(#handlers)*
        }

        // Self-register the plugin with the host: the registration is collected by `inventory`
        // when the crate is linked, so no central plugin list has to be maintained.
        ::zeta_plugin::inventory::submit! {
            ::zeta_plugin::PluginRegistration::new::<#self_ty>(#name)
        }

        #inherent
    }
}

/// The declarative registration statements, in registration order: `urls`, `messages`, then the
/// commands in declaration order, each wrapped in its `register_if` condition when present.
fn registrations(plugin: &Plugin) -> Vec<TokenStream> {
    let mut registrations: Vec<TokenStream> = Vec::new();

    match &plugin.urls {
        Some(UrlsArgs::Any) => {
            registrations.push(quote! {
                subscriptions.urls(::zeta_plugin::UrlScope::Any);
            });
        }
        Some(UrlsArgs::Hosts(hosts)) => {
            registrations.push(quote! {
                subscriptions.urls(::zeta_plugin::UrlScope::Hosts(#hosts));
            });
        }
        None => {}
    }

    if plugin.messages {
        registrations.push(quote! {
            subscriptions.receive_message();
        });
    }

    for command in &plugin.commands {
        let spec = &command.spec;
        let cfg = &command.cfg;
        let registration = quote! { #(#cfg)* subscriptions.command(#spec); };

        registrations.push(
            command
                .args
                .register_if
                .as_ref()
                .map_or(registration, |condition| {
                    quote! { #(#cfg)* if #condition { subscriptions.command(#spec); } }
                }),
        );
    }

    registrations
}

/// The generated constructor's parameter names. Parameters the body does not use are
/// underscore-prefixed to stay warning-free: the constructor call consumes the inherent `fn
/// new`'s parameters, `register_if` conditions may read `settings`, and the registrations use
/// `subscriptions`.
fn constructor_parameters(
    plugin: &Plugin,
    uses_subscriptions: bool,
) -> (TokenStream, TokenStream, TokenStream) {
    let uses_context = plugin.constructor;
    let uses_settings = plugin.constructor
        || plugin
            .commands
            .iter()
            .any(|command| command.args.register_if.is_some());
    let uses_subscriptions = plugin.constructor || uses_subscriptions;

    (
        parameter_name("ctx", uses_context),
        parameter_name("settings", uses_settings),
        parameter_name("subscriptions", uses_subscriptions),
    )
}

/// The parameter's name, underscore-prefixed when unused.
fn parameter_name(name: &str, used: bool) -> TokenStream {
    if used {
        format_ident!("{name}").to_token_stream()
    } else {
        format_ident!("_{name}").to_token_stream()
    }
}

/// The dispatch arm for one `#[command]` method: match the event's trigger, parse the arguments
/// with the chosen helper (replying with the usage lines on failure), and call the method.
///
/// The dispatch is by trigger rather than by matching the event's `CommandSpec` against the
/// constant: a spec created with `with_args` holds a function pointer, and constants containing
/// function pointers are rejected as match patterns.
fn dispatch_arm(command: &Command, default_label: &syn::LitStr) -> TokenStream {
    let args = &command.args;
    let method = &command.method;
    let cfg = &command.cfg;
    let trigger = &args.trigger;
    let label = args.label.as_ref().unwrap_or(default_label);

    let mut arguments: Vec<TokenStream> =
        vec![quote!(ctx), quote!(client), quote!(command.channel())];
    if args.event {
        arguments.push(quote!(command));
    }

    let Some(options) = &args.args else {
        return quote! {
            #(#cfg)*
            #trigger => {
                self.#method(#(#arguments),*).await?;
            }
        };
    };

    let parse = if args.words {
        quote!(parse_words_or_usage)
    } else {
        quote!(parse_args_or_usage)
    };
    arguments.push(quote!(opts));

    quote! {
        #(#cfg)*
        #trigger => {
            let Some(opts) = ::zeta_plugin::#parse::<#options>(
                client,
                command,
                |line: &str| ::zeta_plugin::reply(#label, line),
            )? else {
                return ::core::result::Result::Ok(());
            };

            self.#method(#(#arguments),*).await?;
        }
    }
}

/// The trait handler forwarding to one `#[handler(kind)]` method.
fn handler_impl(handler: &Handler) -> TokenStream {
    let kind = handler.kind;
    let method = &handler.method;
    let cfg = &handler.cfg;
    let trait_method = format_ident!("{}", kind.trait_method());
    let receiver = if kind.mutable_receiver() {
        quote!(&mut self)
    } else {
        quote!(&self)
    };

    let Some((parameter, event)) = kind.event_parameter() else {
        return quote! {
            #(#cfg)*
            async fn #trait_method(
                #receiver,
                ctx: &::zeta_plugin::Context,
                client: &::zeta_plugin::Client,
            ) -> ::core::result::Result<(), ::zeta_plugin::Error> {
                self.#method(ctx, client).await
            }
        };
    };

    let parameter = format_ident!("{parameter}");
    let event = format_ident!("{event}");

    quote! {
        #(#cfg)*
        async fn #trait_method(
            #receiver,
            ctx: &::zeta_plugin::Context,
            client: &::zeta_plugin::Client,
            #parameter: &::zeta_plugin::#event,
        ) -> ::core::result::Result<(), ::zeta_plugin::Error> {
            self.#method(ctx, client, #parameter).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    fn plugin_args() -> PluginArgs {
        PluginArgs {
            name: parse_quote!("test"),
            settings: None,
            urls: None,
            messages: false,
        }
    }

    fn command_args(options: Option<Type>, words: bool) -> CommandArgs {
        CommandArgs {
            trigger: parse_quote!(".test"),
            description: parse_quote!("desc"),
            args: options,
            words,
            spec: None,
            label: None,
            event: false,
            register_if: None,
        }
    }

    fn command_method() -> ImplItemFn {
        parse_quote! {
            async fn test(&self, _: &Context, _: &Client, _: &str) -> Result<(), Error> {
                Ok(())
            }
        }
    }

    #[test]
    fn missing_options_parameter_is_reported() {
        let method = command_method();
        let error = validate_command(&command_args(Some(parse_quote!(Opts)), false), &method)
            .expect_err("a missing options parameter must be rejected");

        assert!(error.to_string().contains("opts"), "{error}");
    }

    #[test]
    fn words_requires_args() {
        let method = command_method();
        let error = validate_command(&command_args(None, true), &method)
            .expect_err("`words` without `args` must be rejected");

        assert!(error.to_string().contains("words"), "{error}");
    }

    #[test]
    fn words_with_args_is_valid() {
        let method: ImplItemFn = parse_quote! {
            async fn test(
                &self,
                _: &Context,
                _: &Client,
                _: &str,
                _opts: Opts,
            ) -> Result<(), Error> {
                Ok(())
            }
        };

        assert!(validate_command(&command_args(Some(parse_quote!(Opts)), true), &method).is_ok());
    }

    #[test]
    fn helper_new_is_not_the_plugin_constructor() {
        let inherent: syn::ItemImpl = parse_quote! {
            impl Plugin {
                fn new() -> Self {
                    Self
                }

                #[command(".test", "desc")]
                async fn test(
                    &self,
                    _: &Context,
                    _: &Client,
                    _: &str,
                ) -> Result<(), Error> {
                    Ok(())
                }
            }
        };

        let plugin = classify(&plugin_args(), &inherent).expect("the impl should classify");

        assert!(!plugin.constructor);
    }

    #[test]
    fn cfg_attributes_are_copied_to_commands() {
        let inherent: syn::ItemImpl = parse_quote! {
            impl Plugin {
                #[cfg(feature = "extra")]
                #[command(".test", "desc")]
                async fn test(
                    &self,
                    _: &Context,
                    _: &Client,
                    _: &str,
                ) -> Result<(), Error> {
                    Ok(())
                }
            }
        };

        let plugin = classify(&plugin_args(), &inherent).expect("the impl should classify");

        assert_eq!(plugin.commands[0].cfg.len(), 1);
    }

    #[test]
    fn duplicate_handler_kinds_are_rejected() {
        let inherent: syn::ItemImpl = parse_quote! {
            impl Plugin {
                #[handler(url)]
                async fn first(
                    &self,
                    _: &Context,
                    _: &Client,
                    _: &UrlEvent,
                ) -> Result<(), Error> {
                    Ok(())
                }

                #[handler(url)]
                async fn second(
                    &self,
                    _: &Context,
                    _: &Client,
                    _: &UrlEvent,
                ) -> Result<(), Error> {
                    Ok(())
                }
            }
        };

        let error = classify(&plugin_args(), &inherent)
            .err()
            .expect("two methods cannot handle the same event kind");

        assert!(error.to_string().contains("duplicate"), "{error}");
    }
}
