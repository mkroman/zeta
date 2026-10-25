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

/// The static description of one [`HandlerKind`].
///
/// Keeping the keyword, the forwarded trait method, and the event parameter together means a new
/// event kind is a single match arm, and the parser and the diagnostics read the same keyword.
struct KindSpec {
    /// The kind as written in `#[handler(kind)]`.
    keyword: &'static str,
    /// The trait method the handler is forwarded through.
    trait_method: &'static str,
    /// The trait method's event parameter: its name and the event type at the `zeta_plugin`
    /// root. `None` for `loaded` and `shutdown`, which take no event.
    event: Option<(&'static str, &'static str)>,
}

impl HandlerKind {
    /// Every kind, in the order diagnostics list them.
    const ALL: &[Self] = &[
        Self::Command,
        Self::Url,
        Self::Message,
        Self::Join,
        Self::Part,
        Self::Quit,
        Self::Nick,
        Self::Kick,
        Self::Ctcp,
        Self::Raw,
        Self::Loaded,
        Self::Shutdown,
    ];

    /// The kind's keyword, trait method, and event parameter.
    const fn spec(self) -> KindSpec {
        match self {
            Self::Command => KindSpec {
                keyword: "command",
                trait_method: "handle_command",
                event: Some(("command", "CommandEvent")),
            },
            Self::Url => KindSpec {
                keyword: "url",
                trait_method: "handle_url",
                event: Some(("url", "UrlEvent")),
            },
            Self::Message => KindSpec {
                keyword: "message",
                trait_method: "handle_message",
                event: Some(("message", "MessageEvent")),
            },
            Self::Join => KindSpec {
                keyword: "join",
                trait_method: "handle_join",
                event: Some(("join", "JoinEvent")),
            },
            Self::Part => KindSpec {
                keyword: "part",
                trait_method: "handle_part",
                event: Some(("part", "PartEvent")),
            },
            Self::Quit => KindSpec {
                keyword: "quit",
                trait_method: "handle_quit",
                event: Some(("quit", "QuitEvent")),
            },
            Self::Nick => KindSpec {
                keyword: "nick",
                trait_method: "handle_nick",
                event: Some(("nick", "NickEvent")),
            },
            Self::Kick => KindSpec {
                keyword: "kick",
                trait_method: "handle_kick",
                event: Some(("kick", "KickEvent")),
            },
            Self::Ctcp => KindSpec {
                keyword: "ctcp",
                trait_method: "handle_ctcp",
                event: Some(("ctcp", "CtcpEvent")),
            },
            Self::Raw => KindSpec {
                keyword: "raw",
                trait_method: "handle_raw",
                event: Some(("raw", "RawEvent")),
            },
            Self::Loaded => KindSpec {
                keyword: "loaded",
                trait_method: "loaded",
                event: None,
            },
            Self::Shutdown => KindSpec {
                keyword: "shutdown",
                trait_method: "shutdown",
                event: None,
            },
        }
    }

    /// Parses the `kind` argument of `#[handler(kind)]`.
    fn from_ident(ident: &Ident) -> Option<Self> {
        Self::ALL.iter().copied().find(|kind| ident == kind.name())
    }

    /// The error for an unrecognized `kind`, listing the accepted ones.
    fn unknown_kind(ident: &Ident) -> syn::Error {
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
    const fn name(self) -> &'static str {
        self.spec().keyword
    }

    /// The trait method the handler is forwarded through.
    const fn trait_method(self) -> &'static str {
        self.spec().trait_method
    }

    /// The trait method's event parameter, or `None` for `loaded` and `shutdown`.
    const fn event_parameter(self) -> Option<(&'static str, &'static str)> {
        self.spec().event
    }

    /// Whether the trait handler takes `&mut self`.
    ///
    /// Only the eventless lifecycle hooks do; every event handler takes `&self`.
    const fn mutable_receiver(self) -> bool {
        self.spec().event.is_none()
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

        let (command, handler) = method_annotations(method)?;

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

/// One method's parsed `#[command]` annotation and the span of its attribute.
type CommandAnnotation = Option<(CommandArgs, Span)>;

/// One method's parsed `#[handler(kind)]` annotation and the span of its attribute.
type HandlerAnnotation = Option<(HandlerKind, Span)>;

/// Reads one method's `#[command]` and `#[handler]` annotations, rejecting duplicates.
fn method_annotations(method: &ImplItemFn) -> syn::Result<(CommandAnnotation, HandlerAnnotation)> {
    let mut command: CommandAnnotation = None;
    let mut handler: HandlerAnnotation = None;

    for attribute in &method.attrs {
        if attribute.path().is_ident("command") {
            if command.is_some() {
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

            let kind_ident: Ident = attribute
                .parse_args_with(|input: syn::parse::ParseStream<'_>| input.parse::<Ident>())?;
            let kind = HandlerKind::from_ident(&kind_ident)
                .ok_or_else(|| HandlerKind::unknown_kind(&kind_ident))?;

            handler = Some((kind, attribute.span()));
        }
    }

    Ok((command, handler))
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
    if !is_constructor(method)? {
        return Ok(());
    }

    if !cfg_attributes(method).is_empty() {
        return Err(syn::Error::new(
            method.sig.ident.span(),
            "the plugin constructor cannot be gated with `#[cfg]`; the generated `Plugin::new` \
             always calls it",
        ));
    }

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

/// Whether the method is the inherent plugin constructor: an associated `fn new` with exactly
/// three typed parameters and the trait's signature.
///
/// Other `new` methods are ordinary inherent methods, so helper constructors (e.g. `fn new() ->
/// Self`) can live beside the plugin impl.
fn is_constructor(method: &ImplItemFn) -> syn::Result<bool> {
    if method.sig.ident != "new"
        || method
            .sig
            .inputs
            .iter()
            .any(|input| matches!(input, syn::FnArg::Receiver(_)))
        || typed_parameters(method).len() != 3
    {
        return Ok(false);
    }

    validate_constructor(method)?;

    Ok(true)
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
        validate_options(options, typed[options_index].ty.as_ref())?;
    }

    if let Some(extra) = typed.get(required) {
        return Err(syn::Error::new(
            extra.ty.span(),
            "unexpected parameter in a command handler",
        ));
    }

    expect_result(signature, ExpectedFirst::Unit, "command handlers")
}

/// Checks that the command's options parameter is the type declared by `args` and taken by
/// value, not by reference.
fn validate_options(declared: &Type, actual: &Type) -> syn::Result<()> {
    if matches!(actual, Type::Reference(_)) {
        return Err(syn::Error::new(
            actual.span(),
            format!(
                "command options are taken by value: `{}`",
                display_type(declared)
            ),
        ));
    }

    if last_path_ident(declared) != last_path_ident(actual) {
        return Err(syn::Error::new(
            actual.span(),
            format!(
                "the command options parameter must have type `{}`",
                display_type(declared)
            ),
        ));
    }

    Ok(())
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
///
/// `is_constructor` has already checked the name, the absence of a receiver, and the parameter
/// count; this validates the three parameter types and the return type.
fn validate_constructor(method: &ImplItemFn) -> syn::Result<()> {
    let signature = &method.sig;
    let name = &signature.ident;

    if signature.asyncness.is_some() {
        return Err(syn::Error::new(
            name.span(),
            "the plugin constructor must not be `async`",
        ));
    }

    let typed = typed_parameters(method);

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

impl ExpectedFirst {
    /// The first `Result` generic argument, as written in the diagnostics.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Unit => "()",
            Self::Self_ => "Self",
        }
    }
}

/// Checks that a signature returns `Result<first, _>`.
fn expect_result(
    signature: &syn::Signature,
    first: ExpectedFirst,
    subject: &str,
) -> syn::Result<()> {
    let expected = first.as_str();

    let syn::ReturnType::Type(_, output) = &signature.output else {
        return Err(syn::Error::new(
            signature.ident.span(),
            format!("{subject} must return `Result<{expected}, Error>`"),
        ));
    };

    let Type::Path(path) = output.as_ref() else {
        return Err(unexpected_return_type(output.span(), subject, first));
    };
    let Some(segment) = path.path.segments.last() else {
        return Err(unexpected_return_type(output.span(), subject, first));
    };
    if segment.ident != "Result" {
        return Err(unexpected_return_type(output.span(), subject, first));
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Err(unexpected_return_type(output.span(), subject, first));
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
        _ => Err(unexpected_return_type(output.span(), subject, first)),
    }
}

/// The "must return `Result<…>`" error for an unexpected return type.
fn unexpected_return_type(span: Span, subject: &str, first: ExpectedFirst) -> syn::Error {
    let expected = first.as_str();

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

    let constants = command_constants(plugin);
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

    let handle_command = handle_command_impl(plugin);
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

/// The module-level `CommandSpec` constants, one per `#[command]` method.
fn command_constants(plugin: &Plugin) -> impl Iterator<Item = TokenStream> + '_ {
    plugin.commands.iter().map(|command| {
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
    })
}

/// The generated `handle_command`, or `None` for a plugin with no `#[command]` methods.
fn handle_command_impl(plugin: &Plugin) -> Option<TokenStream> {
    if plugin.commands.is_empty() {
        return None;
    }

    let arms = plugin
        .commands
        .iter()
        .map(|command| dispatch_arm(command, &plugin.name));

    Some(quote! {
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
    })
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
    has_registrations: bool,
) -> (TokenStream, TokenStream, TokenStream) {
    let uses_context = plugin.constructor;
    let uses_settings = plugin.constructor
        || plugin
            .commands
            .iter()
            .any(|command| command.args.register_if.is_some());
    let uses_subscriptions = plugin.constructor || has_registrations;

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

    // With `args`, parse the options first and reply with the usage lines when they are
    // malformed; the parsed value is the method's last argument.
    let parse = args.args.as_ref().map_or_else(TokenStream::new, |options| {
        let parse = if args.words {
            quote!(parse_words_or_usage)
        } else {
            quote!(parse_args_or_usage)
        };
        arguments.push(quote!(opts));

        quote! {
            let Some(opts) = ::zeta_plugin::#parse::<#options>(
                client,
                command,
                |line: &str| ::zeta_plugin::reply(#label, line),
            )? else {
                return ::core::result::Result::Ok(());
            };
        }
    });

    quote! {
        #(#cfg)*
        #trigger => {
            #parse
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

    // Event handlers declare the event parameter; the lifecycle hooks take none.
    let (event_parameter, event_arguments) = kind.event_parameter().map_or_else(
        || (TokenStream::new(), TokenStream::new()),
        |(parameter, event)| {
            let parameter = format_ident!("{parameter}");
            let event = format_ident!("{event}");

            (
                quote!(, #parameter: &::zeta_plugin::#event),
                quote!(, #parameter),
            )
        },
    );

    quote! {
        #(#cfg)*
        async fn #trait_method(
            #receiver,
            ctx: &::zeta_plugin::Context,
            client: &::zeta_plugin::Client
            #event_parameter,
        ) -> ::core::result::Result<(), ::zeta_plugin::Error> {
            self.#method(ctx, client #event_arguments).await
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

    /// Classifies a parsed `impl` block with the default plugin arguments.
    fn classify_source(source: &str) -> syn::Result<Plugin> {
        let inherent: syn::ItemImpl = syn::parse_str(source).expect("a parsable impl block");

        classify(&plugin_args(), &inherent)
    }

    #[test]
    fn trait_impls_are_rejected() {
        let error = classify_source("impl Plugin for Thing { fn helper() {} }")
            .err()
            .expect("a trait implementation must be rejected");

        assert!(
            error.to_string().contains("trait implementation"),
            "{error}"
        );
    }

    #[test]
    fn generic_impls_are_rejected() {
        let error = classify_source("impl<T> Plugin<T> { fn helper(&self) {} }")
            .err()
            .expect("a generic impl block must be rejected");

        assert!(error.to_string().contains("generic impl"), "{error}");
    }

    #[test]
    fn duplicate_command_triggers_are_rejected() {
        let error = classify_source(
            r#"
            impl Plugin {
                #[command(".test", "one")]
                async fn one(&self, _: &Context, _: &Client, _: &str) -> Result<(), Error> {
                    Ok(())
                }

                #[command(".test", "two")]
                async fn two(&self, _: &Context, _: &Client, _: &str) -> Result<(), Error> {
                    Ok(())
                }
            }
            "#,
        )
        .err()
        .expect("two commands cannot share a trigger");

        assert!(
            error.to_string().contains("duplicate command trigger"),
            "{error}"
        );
    }

    #[test]
    fn duplicate_command_constants_are_rejected() {
        let error = classify_source(
            r#"
            impl Plugin {
                #[command(".one", "one", spec = SHARED)]
                async fn one(&self, _: &Context, _: &Client, _: &str) -> Result<(), Error> {
                    Ok(())
                }

                #[command(".two", "two", spec = SHARED)]
                async fn two(&self, _: &Context, _: &Client, _: &str) -> Result<(), Error> {
                    Ok(())
                }
            }
            "#,
        )
        .err()
        .expect("two commands cannot share a constant name");

        assert!(
            error.to_string().contains("duplicate command constant"),
            "{error}"
        );
    }

    #[test]
    fn commands_cannot_be_combined_with_a_command_handler() {
        let error = classify_source(
            r#"
            impl Plugin {
                #[command(".test", "desc")]
                async fn test(&self, _: &Context, _: &Client, _: &str) -> Result<(), Error> {
                    Ok(())
                }

                #[handler(command)]
                async fn handler(
                    &self,
                    _: &Context,
                    _: &Client,
                    _: &CommandEvent,
                ) -> Result<(), Error> {
                    Ok(())
                }
            }
            "#,
        )
        .err()
        .expect("both would implement `handle_command`");

        assert!(error.to_string().contains("cannot combine"), "{error}");
    }

    #[test]
    fn commands_must_be_async() {
        let error = classify_source(
            r#"
            impl Plugin {
                #[command(".test", "desc")]
                fn test(&self, _: &Context, _: &Client, _: &str) -> Result<(), Error> {
                    Ok(())
                }
            }
            "#,
        )
        .err()
        .expect("a synchronous command handler must be rejected");

        assert!(error.to_string().contains("must be `async`"), "{error}");
    }

    #[test]
    fn command_options_must_be_taken_by_value() {
        let error = classify_source(
            r#"
            impl Plugin {
                #[command(".test", "desc", args = Opts)]
                async fn test(
                    &self,
                    _: &Context,
                    _: &Client,
                    _: &str,
                    _: &Opts,
                ) -> Result<(), Error> {
                    Ok(())
                }
            }
            "#,
        )
        .err()
        .expect("options behind a reference must be rejected");

        assert!(error.to_string().contains("by value"), "{error}");
    }

    #[test]
    fn command_options_must_be_the_declared_type() {
        let error = classify_source(
            r#"
            impl Plugin {
                #[command(".test", "desc", args = Opts)]
                async fn test(
                    &self,
                    _: &Context,
                    _: &Client,
                    _: &str,
                    _: OtherOpts,
                ) -> Result<(), Error> {
                    Ok(())
                }
            }
            "#,
        )
        .err()
        .expect("a different options type must be rejected");

        assert!(error.to_string().contains("must have type"), "{error}");
    }

    #[test]
    fn handler_events_must_match_the_kind() {
        let error = classify_source(
            r"
            impl Plugin {
                #[handler(url)]
                async fn url(
                    &self,
                    _: &Context,
                    _: &Client,
                    _: &MessageEvent,
                ) -> Result<(), Error> {
                    Ok(())
                }
            }
            ",
        )
        .err()
        .expect("a mismatched event type must be rejected");

        assert!(error.to_string().contains("`&UrlEvent`"), "{error}");
    }

    #[test]
    fn lifecycle_handlers_take_a_mutable_receiver() {
        let error = classify_source(
            r"
            impl Plugin {
                #[handler(loaded)]
                async fn loaded(&self, _: &Context, _: &Client) -> Result<(), Error> {
                    Ok(())
                }
            }
            ",
        )
        .err()
        .expect("the trait's lifecycle hooks take `&mut self`");

        assert!(error.to_string().contains("`&mut self`"), "{error}");
    }

    #[test]
    fn unknown_handler_kinds_are_reported() {
        let error = classify_source(
            r"
            impl Plugin {
                #[handler(up)]
                async fn up(&self, _: &Context, _: &Client) -> Result<(), Error> {
                    Ok(())
                }
            }
            ",
        )
        .err()
        .expect("an unknown kind must be rejected");

        assert!(
            error.to_string().contains("unknown event kind `up`"),
            "{error}"
        );
    }

    #[test]
    fn constructors_must_not_be_async() {
        let error = classify_source(
            r"
            impl Plugin {
                async fn new(
                    _: &Context,
                    _: &Settings,
                    _: &mut Subscriptions,
                ) -> Result<Self, Error> {
                    todo!()
                }
            }
            ",
        )
        .err()
        .expect("an async constructor must be rejected");

        assert!(error.to_string().contains("must not be `async`"), "{error}");
    }

    #[test]
    fn constructor_settings_are_shared() {
        let error = classify_source(
            r"
            impl Plugin {
                fn new(
                    _: &Context,
                    _: &mut Settings,
                    _: &mut Subscriptions,
                ) -> Result<Self, Error> {
                    todo!()
                }
            }
            ",
        )
        .err()
        .expect("mutable settings must be rejected");

        assert!(error.to_string().contains("immutable reference"), "{error}");
    }

    #[test]
    fn annotations_are_stripped_and_lints_allowed() {
        let mut inherent: syn::ItemImpl = parse_quote! {
            impl Plugin {
                #[command(".test", "desc")]
                async fn test(&self, _: &Context, _: &Client, _: &str) -> Result<(), Error> {
                    Ok(())
                }
            }
        };
        let plugin = classify(&plugin_args(), &inherent).expect("the impl should classify");

        strip_annotations(&plugin, &mut inherent);

        let ImplItem::Fn(method) = &inherent.items[0] else {
            panic!("the impl should still hold the method");
        };
        assert!(
            !method
                .attrs
                .iter()
                .any(|attr| attr.path().is_ident("command"))
        );
        assert!(
            method
                .attrs
                .iter()
                .any(|attr| attr.path().is_ident("allow"))
        );
    }

    #[test]
    fn generated_paths_are_absolute() {
        let inherent: syn::ItemImpl = parse_quote! {
            impl Plugin {
                #[command(".test", "desc", args = Opts)]
                async fn test(
                    &self,
                    _: &Context,
                    _: &Client,
                    _: &str,
                    _: Opts,
                ) -> Result<(), Error> {
                    Ok(())
                }
            }
        };
        let plugin = classify(&plugin_args(), &inherent).expect("the impl should classify");

        let rendered = render(&plugin, &inherent).to_string();

        assert!(rendered.contains("inventory"), "{rendered}");
        for (index, _) in rendered.match_indices("zeta_plugin") {
            assert!(
                rendered[..index].ends_with(":: "),
                "`zeta_plugin` must only be referenced through an absolute path: {rendered}"
            );
        }
    }
}
