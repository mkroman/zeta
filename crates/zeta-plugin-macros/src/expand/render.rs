//! Emits the generated implementation and constants for a classified plugin.
//!
//! Every generated path is absolute (`::zeta_plugin::…`), because a proc macro has no
//! `$crate`; the inherent impl is emitted unchanged beside the generated items.

use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use syn::ImplItem;

use crate::args::UrlsArgs;

use super::model::{Command, ConstructorParameter, Handler, HandlerKind, Plugin};

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
pub(super) fn strip_annotations(plugin: &Plugin, inherent: &mut syn::ItemImpl) {
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
                        )]
                    ));
                } else {
                    method.attrs.push(syn::parse_quote!(
                        #[allow(clippy::unused_async, clippy::unused_async_trait_impl)]
                    ));
                }
            } else if plugin.constructor.is_some() && method.sig.ident == "new" {
                method.attrs.push(syn::parse_quote!(
                    #[allow(clippy::unnecessary_wraps, clippy::trivially_copy_pass_by_ref)]
                ));
            }
        }
    }
}

/// Renders the macro output: the `CommandSpec` constants, the `Plugin` implementation, and the
/// stripped inherent impl.
pub(super) fn render(plugin: &Plugin, inherent: &syn::ItemImpl) -> TokenStream {
    let self_ty = &inherent.self_ty;
    let name = &plugin.name;
    let settings = plugin.settings.as_ref().map_or_else(
        || quote!(::zeta_plugin::NoSettings),
        |settings| quote!(#settings),
    );

    let constants = command_constants(plugin);
    let registrations = registrations(plugin);
    let preferences = plugin
        .preferences
        .as_ref()
        .map_or_else(TokenStream::new, |preferences| {
            quote! {
                fn preferences() -> ::std::vec::Vec<::zeta_plugin::accounts::PreferenceSpec> {
                    <#preferences as ::zeta_plugin::accounts::PreferenceSet>::preferences(
                        <Self as ::zeta_plugin::Registered>::NAME,
                    )
                }
            }
        });
    let constructor_tail = plugin.constructor.as_ref().map_or_else(
        || {
            quote!(::core::result::Result::Ok(
                <Self as ::core::default::Default>::default()
            ))
        },
        |constructor| {
            let arguments = constructor.arguments();

            quote!(Self::new(#arguments))
        },
    );
    let (context_parameter, settings_parameter, subscriptions_parameter) =
        constructor_parameters(plugin, !registrations.is_empty());

    let handle = handle_impl(plugin);
    let lifecycle = plugin
        .handlers
        .iter()
        .filter(|handler| handler.kind.is_lifecycle())
        .map(lifecycle_impl);

    quote! {
        #(#constants)*

        impl ::zeta_plugin::Plugin for #self_ty {
            #handle

            #(#lifecycle)*
        }

        /// The construction half of the plugin: name, settings and constructor.
        impl ::zeta_plugin::Registered for #self_ty {
            const NAME: &'static str = #name;
            type Settings = #settings;
            #preferences

            #[allow(unused_variables)]
            fn new(
                #context_parameter: &::zeta_plugin::Context,
                #settings_parameter: &#settings,
                #subscriptions_parameter: &mut ::zeta_plugin::Subscriptions,
            ) -> ::core::result::Result<Self, ::zeta_plugin::Error> {
                #(#registrations)*

                #constructor_tail
            }
        }

        #inherent
    }
}

/// The module-level `CommandSpec` constants, one per `#[command]` method.
fn command_constants(plugin: &Plugin) -> impl Iterator<Item = TokenStream> {
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

/// The generated `Plugin::handle`: one match over the event, routing each kind the plugin
/// handles to its annotated method, plus the command dispatch for `#[command]` methods.
///
/// The match lives inside an `async` block, so every arm contributes to one future type — the
/// arms await different handler futures, which a plain `match` in return position cannot
/// unify. The future borrows the plugin and the event and is `Send`, so the runtime awaits it
/// inline without boxing.
fn handle_impl(plugin: &Plugin) -> TokenStream {
    let command = command_arm(plugin);
    let handlers = plugin
        .handlers
        .iter()
        .filter(|handler| !handler.kind.is_lifecycle())
        .map(forward_arm);

    quote! {
        #[allow(unused_variables)]
        fn handle<'a>(
            &'a self,
            ctx: &'a ::zeta_plugin::Context,
            client: &'a ::zeta_plugin::Client,
            event: &'a ::zeta_plugin::Event,
        ) -> impl ::core::future::Future<
            Output = ::core::result::Result<(), ::zeta_plugin::Error>,
        > + ::core::marker::Send + 'a {
            async move {
                match event {
                    #command
                    #(#handlers)*
                    _ => {}
                }

                ::core::result::Result::Ok(())
            }
        }
    }
}

/// The `Event::Command` arm: the trigger dispatch for `#[command]` methods, or the forward to a
/// `#[handler(command)]` method. `None` when the plugin has neither.
fn command_arm(plugin: &Plugin) -> Option<TokenStream> {
    if let Some(handler) = plugin
        .handlers
        .iter()
        .find(|handler| handler.kind == HandlerKind::Command)
    {
        let cfg = &handler.cfg;
        let method = &handler.method;

        return Some(quote! {
            #(#cfg)*
            ::zeta_plugin::Event::Command(command) => {
                self.#method(ctx, client, command).await?;
            }
        });
    }

    if plugin.commands.is_empty() {
        return None;
    }

    let arms = plugin
        .commands
        .iter()
        .map(|command| dispatch_arm(command, &plugin.name));

    Some(quote! {
        ::zeta_plugin::Event::Command(command) => {
            match command.spec.trigger() {
                #(#arms)*
                _ => {}
            }
        }
    })
}

/// The `Event::K` arm forwarding one `#[handler(kind)]` event method.
fn forward_arm(handler: &Handler) -> TokenStream {
    let kind = handler.kind;
    let method = &handler.method;
    let cfg = &handler.cfg;
    let variant = kind.event_variant();
    let parameter = format_ident!("{}", kind.name());

    quote! {
        #(#cfg)*
        ::zeta_plugin::Event::#variant(#parameter) => {
            self.#method(ctx, client, #parameter).await?;
        }
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

    // The annotations are authoritative: a handler for an event kind without a container
    // attribute (all kinds but URLs, which need a host scope) registers its subscription here,
    // so a handler cannot silently never fire.
    let handles_messages = plugin.messages
        || plugin
            .handlers
            .iter()
            .any(|handler| handler.kind == HandlerKind::Message);

    if handles_messages {
        registrations.push(quote! {
            subscriptions.receive_message();
        });
    }

    for handler in &plugin.handlers {
        let Some(subscription) = handler.kind.subscription() else {
            continue;
        };

        // Messages are registered above so the `messages` attribute and the handler cannot
        // double-register; URLs are registered by `urls(...)`, which also carries the scope.
        if matches!(
            handler.kind,
            HandlerKind::Message | HandlerKind::Url | HandlerKind::Command
        ) {
            continue;
        }

        let cfg = &handler.cfg;

        registrations.push(quote! { #(#cfg)* subscriptions.#subscription(); });
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
    let uses_context = plugin
        .constructor
        .as_ref()
        .is_some_and(|constructor| constructor.takes(ConstructorParameter::Context));
    let uses_settings = plugin
        .constructor
        .as_ref()
        .is_some_and(|constructor| constructor.takes(ConstructorParameter::Settings))
        || plugin
            .commands
            .iter()
            .any(|command| command.args.register_if.is_some());
    let uses_subscriptions = plugin
        .constructor
        .as_ref()
        .is_some_and(|constructor| constructor.takes(ConstructorParameter::Subscriptions))
        || has_registrations;

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
/// with the chosen helper (replying with the usage lines on failure), and call the method with
/// the channel only when it declares that parameter.
///
/// The dispatch is by trigger rather than by matching the event's `CommandSpec` against the
/// constant: a spec created with `with_args` holds a function pointer, and constants containing
/// function pointers are rejected as match patterns.
pub(super) fn dispatch_arm(command: &Command, default_label: &syn::LitStr) -> TokenStream {
    let args = &command.args;
    let method = &command.method;
    let cfg = &command.cfg;
    let trigger = &args.trigger;
    let label = args.label.as_ref().unwrap_or(default_label);

    let mut arguments: Vec<TokenStream> = vec![quote!(ctx), quote!(client)];
    if command.channel {
        arguments.push(quote!(command.channel()));
    }
    if args.event {
        arguments.push(quote!(command));
    }

    // A declared `usage` (the complete line) or `usage_hint` (the arguments only) replaces the
    // method call when the command has no arguments, before either the option parser or the
    // handler runs.
    let usage_reply = match (&args.usage, &args.usage_hint) {
        (Some(usage), _) => Some(quote!(#usage)),
        (None, Some(hint)) => Some(quote!(::zeta_plugin::usage_line(#trigger, #hint))),
        (None, None) => None,
    };
    let usage = usage_reply.map(|line| {
        quote! {
            if command.args().trim().is_empty() {
                ::zeta_plugin::send_reply(
                    client,
                    command.channel(),
                    ::zeta_plugin::plain_reply(#line),
                )?;

                return ::core::result::Result::Ok(());
            }
        }
    });

    // With `args`, parse the options first and reply with the usage lines when they are
    // malformed; the parsed value is the method's last argument.
    let parse = args.args.as_ref().map_or_else(TokenStream::new, |options| {
        let mode = if args.words {
            quote!(::zeta_plugin::ArgsMode::Words)
        } else {
            quote!(::zeta_plugin::ArgsMode::Shell)
        };
        arguments.push(quote!(opts));

        quote! {
            let Some(opts) = ::zeta_plugin::parse_or_usage::<#options>(
                client,
                command,
                #mode,
                |line: &str| ::zeta_plugin::labeled_reply(#label, line),
            )? else {
                return ::core::result::Result::Ok(());
            };
        }
    });

    quote! {
        #(#cfg)*
        #trigger => {
            #usage
            #parse
            self.#method(#(#arguments),*).await?;
        }
    }
}

/// The generated `loaded` or `shutdown` implementation forwarding to one `#[handler(kind)]`
/// method.
///
/// The inherent method shadows the trait method here, so a handler named `loaded` forwards to
/// itself rather than recursing.
fn lifecycle_impl(handler: &Handler) -> TokenStream {
    let method = &handler.method;
    let cfg = &handler.cfg;
    let name = format_ident!("{}", handler.kind.name());

    quote! {
        #(#cfg)*
        fn #name<'a>(
            &'a mut self,
            ctx: &'a ::zeta_plugin::Context,
            client: &'a ::zeta_plugin::Client,
        ) -> impl ::core::future::Future<
            Output = ::core::result::Result<(), ::zeta_plugin::Error>,
        > + ::core::marker::Send + 'a {
            async move { self.#method(ctx, client).await }
        }
    }
}
