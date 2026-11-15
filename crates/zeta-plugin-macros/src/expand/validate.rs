//! The signature and cross-method rules the macro enforces.
//!
//! Each check returns the first error with the span of the item that caused it, so the
//! diagnostic underlines the user's code instead of the whole impl block.

use proc_macro2::Span;
use quote::ToTokens;
use syn::spanned::Spanned;
use syn::{GenericArgument, Ident, ImplItemFn, PathArguments, Type};

use crate::args::CommandArgs;

use super::model::{Command, Handler, HandlerKind};

/// Checks that no two commands share a trigger or a `CommandSpec` constant name.
///
/// Commands are compared regardless of their `#[cfg]` attributes: the macro cannot evaluate
/// them, so mutually exclusive variants need distinct triggers.
pub(super) fn validate_duplicates(commands: &[Command]) -> syn::Result<()> {
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
pub(super) fn validate_conflicts(commands: &[Command], handlers: &[Handler]) -> syn::Result<()> {
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
             generated dispatch and the forwarding handler would both handle \
             `Event::Command`",
        ));
    }

    Ok(())
}

/// Derives the `CommandSpec` constant name from a trigger: uppercased, with every
/// non-alphanumeric character folded to `_`, runs of underscores collapsed, and leading and
/// trailing underscores trimmed (`.dig` → `DIG`, `.yt-dlp` → `YT_DLP`).
pub(crate) fn spec_const_name(trigger: &str, span: Span) -> syn::Result<Ident> {
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
/// [, opts: Opts]) -> Result<(), Error>`, where the channel parameter may only be omitted when
/// `event` is set. Returns whether the method declares the channel parameter.
pub(super) fn validate_command(args: &CommandArgs, method: &ImplItemFn) -> syn::Result<bool> {
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
    let signature_error = || {
        syn::Error::new(
            name.span(),
            format!("command handlers must take `{}`", command_signature(args)),
        )
    };

    if typed.len() < 2 {
        return Err(signature_error());
    }

    expect_borrowed(typed[0].ty.as_ref(), "Context", false)?;
    expect_borrowed(typed[1].ty.as_ref(), "Client", false)?;

    let mut index = 2;

    // Event commands may omit the channel and read it off the event instead. A non-event
    // command always requires it, because that is its only source of the channel.
    let channel = if args.event {
        if typed
            .get(index)
            .is_some_and(|parameter| is_reference_to(parameter.ty.as_ref(), "str"))
        {
            index += 1;

            true
        } else if typed
            .get(index)
            .is_some_and(|parameter| !is_reference_to(parameter.ty.as_ref(), "CommandEvent"))
        {
            // The parameter is neither the optional channel nor the event itself, so the
            // signature is missing a parameter or the channel has the wrong type.
            return Err(signature_error());
        } else {
            false
        }
    } else {
        if !typed
            .get(index)
            .is_some_and(|parameter| is_reference_to(parameter.ty.as_ref(), "str"))
        {
            return Err(signature_error());
        }

        index += 1;

        true
    };

    if args.event {
        if typed.len() <= index {
            return Err(signature_error());
        }

        expect_borrowed(typed[index].ty.as_ref(), "CommandEvent", false)?;
        index += 1;
    }

    if let Some(options) = &args.args {
        if typed.len() <= index {
            return Err(signature_error());
        }

        validate_options(options, typed[index].ty.as_ref())?;
        index += 1;
    }

    if let Some(extra) = typed.get(index) {
        return Err(syn::Error::new(
            extra.ty.span(),
            "unexpected parameter in a command handler",
        ));
    }

    expect_result(signature, ExpectedFirst::Unit, "command handlers")?;

    Ok(channel)
}

/// The command signature convention for a diagnostic, with the event's optional channel and
/// options in brackets.
fn command_signature(args: &CommandArgs) -> String {
    if !args.event {
        let options = if args.args.is_some() { ", opts" } else { "" };

        return format!("(&self, ctx: &Context, client: &Client, channel: &str{options})");
    }

    let options = if args.args.is_some() { " [, opts]" } else { "" };

    format!(
        "(&self, ctx: &Context, client: &Client, [channel: &str,] command: &CommandEvent{options})"
    )
}

/// Whether `ty` is an immutable reference to a type whose last path segment is `expected`.
///
/// The reference must be the only one: the generated call passes a `&T`, so `&&T` does not
/// match and is reported as a signature error. Qualified paths such as `&std::primitive::str`
/// are accepted.
fn is_reference_to(ty: &Type, expected: &str) -> bool {
    matches!(
        ty,
        Type::Reference(reference)
            if reference.mutability.is_none()
                && matches!(
                    reference.elem.as_ref(),
                    Type::Path(path)
                        if path
                            .path
                            .segments
                            .last()
                            .is_some_and(|segment| segment.ident == expected)
                )
    )
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
pub(super) fn validate_handler(kind: HandlerKind, method: &ImplItemFn) -> syn::Result<()> {
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

    let mut expected: Vec<String> = vec!["Context".to_owned(), "Client".to_owned()];
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

/// Validates the recognized plugin constructor's signature: synchronous, returning
/// `Result<Self, Error>`. The parameters themselves are classified by
/// [`constructor_parameter`](super::classify::constructor_parameter).
pub(super) fn validate_constructor(method: &ImplItemFn) -> syn::Result<()> {
    let signature = &method.sig;
    let name = &signature.ident;

    if signature.asyncness.is_some() {
        return Err(syn::Error::new(
            name.span(),
            "the plugin constructor must not be `async`",
        ));
    }

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
            if matches!(
                &receiver.kind,
                syn::ReceiverKind::Reference(_, _, mutability)
                    if mutability.is_some() == mutable
            ) {
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
pub(super) fn typed_parameters(method: &ImplItemFn) -> Vec<&syn::PatType> {
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
pub(super) fn last_path_ident(ty: &Type) -> Option<&Ident> {
    match ty {
        Type::Reference(reference) => last_path_ident(&reference.elem),
        Type::Path(path) => path.path.segments.last().map(|segment| &segment.ident),
        _ => None,
    }
}

/// Renders a type for an error message.
pub(super) fn display_type(ty: &Type) -> String {
    ty.to_token_stream().to_string()
}
