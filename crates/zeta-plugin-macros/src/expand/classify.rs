//! Reads a `#[plugin]` impl block's annotations into the model.
//!
//! Classification parses `#[command]` and `#[handler]` off the block's methods, recognizes
//! the inherent constructor, and rejects the targets the macro cannot implement `Plugin` for.

use proc_macro2::Span;
use syn::parse::Parse;
use syn::spanned::Spanned;
use syn::{Ident, ImplItem, ImplItemFn, Type};

use crate::args::{CommandArgs, PluginArgs};

use super::model::{Command, Constructor, ConstructorParameter, Handler, HandlerKind, Plugin};
use super::validate::{
    display_type, last_path_ident, spec_const_name, typed_parameters, validate_command,
    validate_conflicts, validate_constructor, validate_duplicates, validate_handler,
};

/// Rejects handlers whose subscription the macro cannot derive from the annotation.
///
/// Every other event kind is registered from its `#[handler(kind)]` annotation; a URL handler
/// additionally needs the `urls(...)` attribute, because only the plugin knows which hosts it
/// wants. Without one the handler would compile and never run.
pub(super) fn validate_handler_subscriptions(plugin: &Plugin) -> Option<syn::Error> {
    plugin.handlers.iter().find_map(|handler| {
        (handler.kind == HandlerKind::Url && plugin.urls.is_none()).then(|| {
            syn::Error::new(
                handler.span,
                "a `#[handler(url)]` method needs a `urls(...)` attribute on the impl block to \
                 declare the hosts it receives",
            )
        })
    })
}

/// Rejects trait impls and generic impl blocks, which the macro cannot implement `Plugin` for.
fn reject_target(inherent: &syn::ItemImpl) -> Option<syn::Error> {
    if let Some((trait_path, _)) = &inherent.trait_ {
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
pub(super) fn classify(args: &PluginArgs, inherent: &syn::ItemImpl) -> syn::Result<Plugin> {
    if let Some(error) = reject_target(inherent) {
        return Err(error);
    }

    let mut commands: Vec<Command> = Vec::new();
    let mut handlers: Vec<Handler> = Vec::new();
    let mut constructor = None;

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
                         method is dispatched through the generated `handle`",
                    ));
                }

                let channel = validate_command(&args, method)?;

                let spec = match &args.spec {
                    Some(spec) => spec.clone(),
                    None => spec_const_name(&args.trigger.value(), args.trigger.span())?,
                };

                commands.push(Command {
                    spec,
                    args,
                    method: method.sig.ident.clone(),
                    channel,
                    cfg: cfg_attributes(method),
                });
            }
            None => match handler {
                Some((kind, span)) => {
                    classify_handler(&mut handlers, kind, span, method)?;
                }
                None => classify_constructor(method, args, &mut constructor)?,
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
        preferences: args.preferences.clone(),
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

/// Classifies the inherent constructor when `method` is a candidate `fn new`.
fn classify_constructor(
    method: &ImplItemFn,
    args: &PluginArgs,
    constructor: &mut Option<Constructor>,
) -> syn::Result<()> {
    let Some(recognized) = is_constructor(method, args)? else {
        return Ok(());
    };

    // Only a `#[cfg]` removes the method from the build; a `cfg_attr` that does not expand to
    // `cfg` leaves the generated call valid, so common forms like `cfg_attr(test, allow(..))`
    // stay usable.
    if method
        .attrs
        .iter()
        .any(|attribute| attribute.path().is_ident("cfg"))
    {
        return Err(syn::Error::new(
            method.sig.ident.span(),
            "the plugin constructor cannot be gated with `#[cfg]`; the generated `Registered::new` \
             always calls it",
        ));
    }

    *constructor = Some(Constructor {
        parameters: recognized,
    });

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

/// The recognized parameters of the inherent plugin constructor, or `None` when `new` is an
/// ordinary helper.
///
/// An associated `fn new` taking one to three typed parameters is the constructor when every
/// parameter is `&Context`, an immutable reference to the plugin's declared settings type, or
/// `&mut Subscriptions`, in any order. A `new` with more than three parameters, or with none of
/// those types, stays a helper; a one-to-three-parameter `new` that mixes recognized and
/// unrecognized types is an error, because it is almost always a typo in one of them.
fn is_constructor(
    method: &ImplItemFn,
    args: &PluginArgs,
) -> syn::Result<Option<Vec<ConstructorParameter>>> {
    if method.sig.ident != "new"
        || method
            .sig
            .inputs
            .iter()
            .any(|input| matches!(input, syn::FnArg::Receiver(_)))
    {
        return Ok(None);
    }

    let typed = typed_parameters(method);
    if typed.is_empty() || typed.len() > 3 {
        return Ok(None);
    }

    let settings = settings_type_name(args);
    let classified: Vec<Option<ConstructorParameter>> = typed
        .iter()
        .map(|parameter| constructor_parameter(parameter.ty.as_ref(), &settings))
        .collect();
    let parameters: Vec<ConstructorParameter> = classified.iter().flatten().copied().collect();

    if parameters.is_empty() {
        return Ok(None);
    }

    if let Some(index) = classified.iter().position(Option::is_none) {
        return Err(syn::Error::new(
            typed[index].ty.span(),
            format!(
                "unrecognized plugin constructor parameter; expected `&Context`, `&{settings}`, \
                 or `&mut Subscriptions`, found `{}`",
                display_type(typed[index].ty.as_ref())
            ),
        ));
    }

    for (index, parameter) in parameters.iter().enumerate() {
        if parameters[..index].contains(parameter) {
            return Err(syn::Error::new(
                typed[index].ty.span(),
                format!(
                    "duplicate plugin constructor parameter; `{}` is already declared",
                    parameter.as_str()
                ),
            ));
        }
    }

    validate_constructor(method)?;

    Ok(Some(parameters))
}

/// The last path segment of the plugin's declared settings type, or `NoSettings`.
fn settings_type_name(args: &PluginArgs) -> String {
    args.settings
        .as_ref()
        .and_then(last_path_ident)
        .map_or_else(|| "NoSettings".to_owned(), ToString::to_string)
}

/// Recognizes one plugin constructor parameter by its type.
fn constructor_parameter(ty: &Type, settings: &str) -> Option<ConstructorParameter> {
    let Type::Reference(reference) = ty else {
        return None;
    };

    match (
        last_path_ident(&reference.elem),
        reference.mutability.is_some(),
    ) {
        (Some(ident), false) if ident == "Context" => Some(ConstructorParameter::Context),
        (Some(ident), true) if ident == "Subscriptions" => {
            Some(ConstructorParameter::Subscriptions)
        }
        (Some(ident), false) if ident == settings => Some(ConstructorParameter::Settings),
        _ => None,
    }
}
