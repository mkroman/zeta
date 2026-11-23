use super::*;
use crate::args::{CommandArgs, PluginArgs};
use syn::parse_quote;
use syn::{ImplItem, ImplItemFn, Type};

fn plugin_args() -> PluginArgs {
    PluginArgs {
        name: parse_quote!("test"),
        settings: None,
        urls: None,
        messages: false,
        preferences: None,
    }
}

/// Plugin arguments with a declared settings type, for constructor tests.
fn plugin_args_with_settings(settings: Type) -> PluginArgs {
    PluginArgs {
        settings: Some(settings),
        ..plugin_args()
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
        usage: None,
        usage_hint: None,
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
fn event_commands_may_omit_the_channel() {
    let inherent: syn::ItemImpl = parse_quote! {
        impl Plugin {
            #[command(".test", "desc", event)]
            async fn test(
                &self,
                _: &Context,
                _: &Client,
                _: &CommandEvent,
            ) -> Result<(), Error> {
                Ok(())
            }
        }
    };

    let plugin = classify(&plugin_args(), &inherent).expect("the impl should classify");

    assert!(!plugin.commands[0].channel);
    let arm = dispatch_arm(&plugin.commands[0], &plugin.name).to_string();
    assert!(!arm.contains("channel"), "{arm}");
}

#[test]
fn event_commands_may_declare_the_channel() {
    let inherent: syn::ItemImpl = parse_quote! {
        impl Plugin {
            #[command(".test", "desc", event)]
            async fn test(
                &self,
                _: &Context,
                _: &Client,
                _: &str,
                _: &CommandEvent,
            ) -> Result<(), Error> {
                Ok(())
            }
        }
    };

    let plugin = classify(&plugin_args(), &inherent).expect("the impl should classify");

    assert!(plugin.commands[0].channel);
    let arm = dispatch_arm(&plugin.commands[0], &plugin.name).to_string();
    assert!(arm.contains("channel"), "{arm}");
}

#[test]
fn non_event_commands_require_the_channel() {
    let error = classify_source(
        r#"
            impl Plugin {
                #[command(".test", "desc")]
                async fn test(&self, _: &Context, _: &Client) -> Result<(), Error> {
                    Ok(())
                }
            }
            "#,
    )
    .err()
    .expect("a non-event command needs the channel");

    assert!(error.to_string().contains("channel: &str"), "{error}");
}

#[test]
fn command_options_require_the_channel() {
    let error = classify_source(
        r#"
            impl Plugin {
                #[command(".test", "desc", args = Opts)]
                async fn test(
                    &self,
                    _: &Context,
                    _: &Client,
                    _: Opts,
                ) -> Result<(), Error> {
                    Ok(())
                }
            }
            "#,
    )
    .err()
    .expect("a non-event command with options still needs the channel");

    assert!(error.to_string().contains("channel: &str"), "{error}");
}

#[test]
fn qualified_str_channel_is_accepted() {
    let plugin = classify_source(
        r#"
            impl Plugin {
                #[command(".test", "desc")]
                async fn test(
                    &self,
                    _: &Context,
                    _: &Client,
                    _: &std::primitive::str,
                ) -> Result<(), Error> {
                    Ok(())
                }
            }
            "#,
    )
    .unwrap_or_else(|error| panic!("`&std::primitive::str` is `&str`: {error}"));

    assert!(plugin.commands[0].channel);
}

#[test]
fn event_commands_require_the_event() {
    let error = classify_source(
        r#"
            impl Plugin {
                #[command(".test", "desc", args = Opts, event)]
                async fn test(
                    &self,
                    _: &Context,
                    _: &Client,
                    _: Opts,
                ) -> Result<(), Error> {
                    Ok(())
                }
            }
            "#,
    )
    .err()
    .expect("an event command needs the event parameter");

    assert!(
        error.to_string().contains("command: &CommandEvent"),
        "{error}"
    );
}

#[test]
fn event_commands_require_the_event_after_the_channel() {
    let error = classify_source(
        r#"
            impl Plugin {
                #[command(".test", "desc", event)]
                async fn test(
                    &self,
                    _: &Context,
                    _: &Client,
                    _: &str,
                ) -> Result<(), Error> {
                    Ok(())
                }
            }
            "#,
    )
    .err()
    .expect("an event command needs the event parameter after the channel");

    assert!(
        error.to_string().contains("command: &CommandEvent"),
        "{error}"
    );
}

#[test]
fn event_channels_must_be_str() {
    let error = classify_source(
        r#"
            impl Plugin {
                #[command(".test", "desc", event)]
                async fn test(
                    &self,
                    _: &Context,
                    _: &Client,
                    _: &String,
                    _: &CommandEvent,
                ) -> Result<(), Error> {
                    Ok(())
                }
            }
            "#,
    )
    .err()
    .expect("the channel must be `&str`, not a reference to another type");

    assert!(error.to_string().contains("channel: &str"), "{error}");
}

#[test]
fn double_reference_channels_are_rejected() {
    let error = classify_source(
        r#"
            impl Plugin {
                #[command(".test", "desc")]
                async fn test(
                    &self,
                    _: &Context,
                    _: &Client,
                    _: &&str,
                ) -> Result<(), Error> {
                    Ok(())
                }
            }
            "#,
    )
    .err()
    .expect("the channel is passed as `&str`, not `&&str`");

    assert!(error.to_string().contains("channel: &str"), "{error}");
}

#[test]
fn double_reference_events_are_rejected() {
    let error = classify_source(
        r#"
            impl Plugin {
                #[command(".test", "desc", event)]
                async fn test(
                    &self,
                    _: &Context,
                    _: &Client,
                    _: &&CommandEvent,
                ) -> Result<(), Error> {
                    Ok(())
                }
            }
            "#,
    )
    .err()
    .expect("the event is passed as `&CommandEvent`, not `&&CommandEvent`");

    assert!(
        error.to_string().contains("command: &CommandEvent"),
        "{error}"
    );
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

    assert!(plugin.constructor.is_none());
}

#[test]
fn reference_helper_new_is_not_the_plugin_constructor() {
    let inherent: syn::ItemImpl = parse_quote! {
        impl Plugin {
            fn new(name: &str) -> Self {
                let _ = name;
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

    assert!(plugin.constructor.is_none());
}

#[test]
fn constructor_parameters_must_match_the_settings_type() {
    let error = classify_source(
        r"
            impl Plugin {
                fn new(_: &Context, _: &NotTheSettings) -> Result<Self, Error> {
                    todo!()
                }
            }
            ",
    )
    .err()
    .expect("a reference to an unrelated type is not the settings parameter");

    assert!(
        error
            .to_string()
            .contains("unrecognized plugin constructor parameter"),
        "{error}"
    );
}

/// The recognized parameters of the impl block's constructor.
fn constructor_parameters(source: &str) -> Vec<ConstructorParameter> {
    classify_source(source)
        .expect("the impl should classify")
        .constructor
        .expect("the impl should declare the plugin constructor")
        .parameters
}

#[test]
fn single_parameter_constructor_is_recognized() {
    let inherent: syn::ItemImpl = parse_quote! {
        impl Plugin {
            fn new(_: &Context) -> Result<Self, Error> {
                Ok(Self)
            }
        }
    };
    let plugin = classify(&plugin_args(), &inherent).expect("the impl should classify");
    let constructor = plugin
        .constructor
        .as_ref()
        .expect("the constructor should be recognized");

    assert_eq!(constructor.parameters, vec![ConstructorParameter::Context]);
    let rendered = render(&plugin, &inherent).to_string();
    assert!(rendered.contains("Self :: new (ctx)"), "{rendered}");
}

#[test]
fn two_parameter_constructor_is_recognized_in_declared_order() {
    let inherent: syn::ItemImpl = parse_quote! {
        impl Plugin {
            fn new(_: &NoSettings, _: &Context) -> Result<Self, Error> {
                Ok(Self)
            }
        }
    };
    let plugin = classify(&plugin_args(), &inherent).expect("the impl should classify");
    let constructor = plugin
        .constructor
        .as_ref()
        .expect("the constructor should be recognized");

    assert_eq!(
        constructor.parameters,
        vec![
            ConstructorParameter::Settings,
            ConstructorParameter::Context
        ]
    );
    let rendered = render(&plugin, &inherent).to_string();
    assert!(
        rendered.contains("Self :: new (settings , ctx)"),
        "{rendered}"
    );
}

#[test]
fn three_parameter_constructor_may_declare_any_order() {
    assert_eq!(
        constructor_parameters(
            r"
                impl Plugin {
                    fn new(
                        _: &mut Subscriptions,
                        _: &NoSettings,
                        _: &Context,
                    ) -> Result<Self, Error> {
                        Ok(Self)
                    }
                }
                ",
        ),
        vec![
            ConstructorParameter::Subscriptions,
            ConstructorParameter::Settings,
            ConstructorParameter::Context,
        ]
    );
}

#[test]
fn duplicate_constructor_parameters_are_rejected() {
    let error = classify_source(
        r"
            impl Plugin {
                fn new(_: &Context, _: &Context) -> Result<Self, Error> {
                    Ok(Self)
                }
            }
            ",
    )
    .err()
    .expect("a duplicated parameter must be rejected");

    assert!(
        error
            .to_string()
            .contains("duplicate plugin constructor parameter"),
        "{error}"
    );
}

#[test]
fn unrecognized_parameters_alone_leave_new_a_helper() {
    let plugin = classify_source(
        r"
            impl Plugin {
                fn new(_: u32) -> Self {
                    Self
                }
            }
            ",
    )
    .expect("an unrecognized helper `new` should stay a helper");

    assert!(plugin.constructor.is_none());
}

#[test]
fn mixed_recognized_and_unrecognized_parameters_are_rejected() {
    let error = classify_source(
        r"
            impl Plugin {
                fn new(_: &Context, _: u32) -> Result<Self, Error> {
                    Ok(Self)
                }
            }
            ",
    )
    .err()
    .expect("a typoed constructor parameter must be rejected");

    assert!(
        error
            .to_string()
            .contains("unrecognized plugin constructor parameter"),
        "{error}"
    );
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

#[test]
fn url_handlers_require_a_urls_attribute() {
    let plugin = classify_source(
        r"
            impl Plugin {
                #[handler(url)]
                async fn url(
                    &self,
                    _: &Context,
                    _: &Client,
                    _: &UrlEvent,
                ) -> Result<(), Error> {
                    Ok(())
                }
            }
            ",
    )
    .expect("the impl should classify");

    let error = validate_handler_subscriptions(&plugin)
        .expect("a URL handler without a URL scope must be rejected");

    assert!(error.to_string().contains("urls(...)"), "{error}");
}

#[test]
fn command_usage_replies_before_dispatch() {
    let inherent: syn::ItemImpl = parse_quote! {
        impl Plugin {
            #[command(".test", "desc", event, usage = "Usage: .test <arg>")]
            async fn test(
                &self,
                _: &Context,
                _: &Client,
                _: &CommandEvent,
            ) -> Result<(), Error> {
                Ok(())
            }
        }
    };
    let plugin = classify(&plugin_args(), &inherent).expect("the impl should classify");

    let rendered = render(&plugin, &inherent).to_string();

    assert!(rendered.contains("plain_reply"), "{rendered}");
    assert!(rendered.contains("send_reply"), "{rendered}");
    assert!(rendered.contains("Usage: .test <arg>"), "{rendered}");
}

#[test]
fn command_usage_hint_derives_the_usage_line() {
    let inherent: syn::ItemImpl = parse_quote! {
        impl Plugin {
            #[command(".test", "desc", event, usage_hint = "<arg>")]
            async fn test(
                &self,
                _: &Context,
                _: &Client,
                _: &CommandEvent,
            ) -> Result<(), Error> {
                Ok(())
            }
        }
    };
    let plugin = classify(&plugin_args(), &inherent).expect("the impl should classify");

    let rendered = render(&plugin, &inherent).to_string();

    assert!(rendered.contains("plain_reply"), "{rendered}");
    assert!(rendered.contains("send_reply"), "{rendered}");
    assert!(
        rendered.contains(":: zeta_plugin :: usage_line (\".test\" , \"<arg>\")"),
        "{rendered}"
    );
}

#[test]
fn event_handlers_register_their_own_subscriptions() {
    let source = r"
            impl Plugin {
                #[handler(message)]
                async fn message(
                    &self,
                    _: &Context,
                    _: &Client,
                    _: &MessageEvent,
                ) -> Result<(), Error> {
                    Ok(())
                }

                #[handler(join)]
                async fn join(
                    &self,
                    _: &Context,
                    _: &Client,
                    _: &JoinEvent,
                ) -> Result<(), Error> {
                    Ok(())
                }
            }
            ";
    let inherent: syn::ItemImpl = syn::parse_str(source).expect("a parsable impl block");
    let plugin = classify(&plugin_args(), &inherent).expect("the impl should classify");

    let rendered = render(&plugin, &inherent).to_string();

    assert!(rendered.contains("receive_message"), "{rendered}");
    assert!(rendered.contains("receive_join"), "{rendered}");
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
    .expect("both would handle `Event::Command`");

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
    let inherent: syn::ItemImpl = parse_quote! {
        impl Plugin {
            async fn new(
                _: &Context,
                _: &Settings,
                _: &mut Subscriptions,
            ) -> Result<Self, Error> {
                todo!()
            }
        }
    };
    let error = classify(
        &plugin_args_with_settings(parse_quote!(Settings)),
        &inherent,
    )
    .err()
    .expect("an async constructor must be rejected");

    assert!(error.to_string().contains("must not be `async`"), "{error}");
}

#[test]
fn cfg_attr_on_the_constructor_is_allowed() {
    let plugin = classify_source(
        r"
            impl Plugin {
                #[cfg_attr(test, allow(dead_code))]
                fn new(
                    _: &Context,
                    _: &NoSettings,
                    _: &mut Subscriptions,
                ) -> Result<Self, Error> {
                    Ok(Self)
                }
            }
            ",
    )
    .unwrap_or_else(|error| panic!("a non-gating `cfg_attr` should be allowed: {error}"));

    assert!(plugin.constructor.is_some());
}

#[test]
fn constructor_settings_are_shared() {
    let inherent: syn::ItemImpl = parse_quote! {
        impl Plugin {
            fn new(
                _: &Context,
                _: &mut Settings,
                _: &mut Subscriptions,
            ) -> Result<Self, Error> {
                todo!()
            }
        }
    };
    let error = classify(
        &plugin_args_with_settings(parse_quote!(Settings)),
        &inherent,
    )
    .err()
    .expect("mutable settings must be rejected");

    assert!(error.to_string().contains("`&Settings`"), "{error}");
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

/// The rendered tokens with the whitespace removed, so assertions read as code.
fn compact(tokens: &TokenStream) -> String {
    tokens
        .to_string()
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

/// Asserts that every `zeta_plugin` occurrence is an absolute path.
fn assert_absolute_paths(rendered: &str) {
    for (index, _) in rendered.match_indices("zeta_plugin") {
        assert!(
            rendered[..index].ends_with("::"),
            "`zeta_plugin` must only be referenced through an absolute path: {rendered}"
        );
    }
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
    let args = PluginArgs {
        preferences: Some(parse_quote!(Prefs)),
        ..plugin_args()
    };
    let plugin = classify(&args, &inherent).expect("the impl should classify");

    let rendered = compact(&render(&plugin, &inherent));

    assert!(rendered.contains("fnhandle"), "{rendered}");
    assert_absolute_paths(&rendered);
}

#[test]
fn the_preferences_type_fills_the_registered_method() {
    let inherent: syn::ItemImpl = parse_quote! {
        impl Plugin {}
    };
    let args = PluginArgs {
        preferences: Some(parse_quote!(Prefs)),
        ..plugin_args()
    };
    let plugin = classify(&args, &inherent).expect("the impl should classify");

    let rendered = compact(&render(&plugin, &inherent));

    assert!(
        rendered.contains(
            "fnpreferences()->::std::vec::Vec<::zeta_plugin::accounts::PreferenceSpec>{\
             <Prefsas::zeta_plugin::accounts::PreferenceSet>::preferences(<Selfas\
             ::zeta_plugin::Registered>::NAME,)}"
        ),
        "{rendered}"
    );
}

#[test]
fn a_plugin_without_preferences_declares_none() {
    let inherent: syn::ItemImpl = parse_quote! {
        impl Plugin {}
    };
    let plugin = classify(&plugin_args(), &inherent).expect("the impl should classify");

    let rendered = compact(&render(&plugin, &inherent));

    assert!(!rendered.contains("fnpreferences"), "{rendered}");
    assert!(!rendered.contains("PreferenceSpec"), "{rendered}");
}
