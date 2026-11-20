//! The `#[derive(PreferenceSet)]` expansion.
//!
//! The derive reads a struct of preference fields — every field a `T` or `Option<T>` with a doc
//! comment — and generates the `PreferenceSet` descriptors, the typed reader, and the reader's
//! constructor on the set itself.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::spanned::Spanned;
use syn::{
    Data, DeriveInput, Expr, ExprLit, Field, Fields, GenericArgument, Ident, Lit, LitStr, Meta,
    PathArguments, Type, Visibility,
};

/// One preference field of the set.
struct Preference {
    /// The field's name, which is also the preference key and the reader's method name.
    key: Ident,
    /// The preference's value type: the `T` of a `T` or `Option<T>` field.
    value: Type,
    /// Whether the field is `Option<T>`, i.e. unset unless the user stores a value.
    optional: bool,
    /// The field's doc comment, trimmed and joined into one description.
    description: String,
}

impl Preference {
    /// Reads one named field of the set.
    fn parse(field: &Field) -> syn::Result<Self> {
        let key = field.ident.clone().ok_or_else(|| {
            syn::Error::new(field.span(), "`PreferenceSet` requires named fields")
        })?;

        if let Some(attribute) = super::preference_attribute(&field.attrs)? {
            return Err(syn::Error::new(
                attribute.span(),
                format!(
                    "`#[preference(...)]` is not supported on the preference field `{key}`; the \
                     field's name is the preference key"
                ),
            ));
        }

        let (value, optional) = preference_type(field, &key)?;
        let description = description(field, &key)?;

        Ok(Self {
            key,
            value,
            optional,
            description,
        })
    }

    /// The field's descriptor, with the plugin name taken from `plugin`.
    fn spec(&self, plugin: &TokenStream) -> TokenStream {
        let key = self.key.to_string();
        let value = &self.value;
        let description = &self.description;
        let default = if self.optional {
            quote!(::core::option::Option::None)
        } else {
            quote! {
                ::core::option::Option::Some(
                    <#value as ::zeta_plugin::accounts::PreferenceValue>::as_str(
                        <#value as ::core::default::Default>::default()
                    )
                )
            }
        };

        quote! {
            ::zeta_plugin::accounts::PreferenceSpec {
                plugin: #plugin,
                key: #key,
                kind: ::zeta_plugin::accounts::PreferenceKind::Enum(
                    <#value as ::zeta_plugin::accounts::PreferenceValue>::VALUES
                ),
                description: #description,
                default: #default,
            }
        }
    }

    /// The reader method that reads this preference, with the field's name and the set's
    /// visibility.
    fn reader_method(&self, vis: &Visibility) -> TokenStream {
        let key = &self.key;
        let value = &self.value;
        let spec = self.spec(&quote!(self.plugin));

        if self.optional {
            let docs = LitStr::new(
                &format!(
                    "Reads the `{key}` preference, or `None` when it is unset or the sender is \
                     unidentified.\n\n# Errors\n\nReturns an error when the preference cannot be \
                     read from the database."
                ),
                key.span(),
            );

            quote! {
                #[doc = #docs]
                #vis async fn #key(
                    &self,
                ) -> ::core::result::Result<
                    ::core::option::Option<#value>, ::zeta_plugin::database::DbError
                > {
                    let ::core::option::Option::Some(identity) = self.identity else {
                        return ::core::result::Result::Ok(::core::option::Option::None);
                    };

                    let spec = #spec;
                    let value = self.service.value(identity, &spec).await?;

                    ::core::result::Result::Ok(value.and_then(
                        <#value as ::zeta_plugin::accounts::PreferenceValue>::canonical,
                    ))
                }
            }
        } else {
            let docs = LitStr::new(
                &format!(
                    "Reads the `{key}` preference, falling back to its default when it is unset or \
                     the sender is unidentified.\n\n# Errors\n\nReturns an error when the \
                     preference cannot be read from the database."
                ),
                key.span(),
            );

            quote! {
                #[doc = #docs]
                #vis async fn #key(
                    &self,
                ) -> ::core::result::Result<#value, ::zeta_plugin::database::DbError> {
                    let ::core::option::Option::Some(identity) = self.identity else {
                        return ::core::result::Result::Ok(
                            <#value as ::core::default::Default>::default(),
                        );
                    };

                    let spec = #spec;
                    let value = self.service.value_or_default(identity, &spec).await?;

                    ::core::result::Result::Ok(
                        value
                            .and_then(<#value as ::zeta_plugin::accounts::PreferenceValue>::canonical)
                            .unwrap_or_default(),
                    )
                }
            }
        }
    }
}

/// The field's value type and whether the preference is optional.
fn preference_type(field: &Field, key: &Ident) -> syn::Result<(Type, bool)> {
    let ty = &field.ty;

    let Some(inner) = optional_value(ty)? else {
        ensure_value_type(ty, key)?;

        return Ok((ty.clone(), false));
    };

    if optional_value(inner)?.is_some() {
        return Err(syn::Error::new(
            inner.span(),
            "a preference is set or unset, so `Option<Option<…>>` is not supported",
        ));
    }

    ensure_value_type(inner, key)?;

    Ok((inner.clone(), true))
}

/// The `T` of an `Option<T>` type, or `None` when the type is not an `Option`.
fn optional_value(ty: &Type) -> syn::Result<Option<&Type>> {
    let Type::Path(path) = ty else {
        return Ok(None);
    };
    let Some(segment) = path.path.segments.last() else {
        return Ok(None);
    };
    if segment.ident != "Option" {
        return Ok(None);
    }

    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Err(syn::Error::new(
            ty.span(),
            "`Option` needs a preference value type: `Option<Mode>`",
        ));
    };

    match (arguments.args.len(), arguments.args.first()) {
        (1, Some(GenericArgument::Type(inner))) => Ok(Some(inner)),
        _ => Err(syn::Error::new(
            ty.span(),
            "`Option` takes exactly one preference value type: `Option<Mode>`",
        )),
    }
}

/// Rejects a value type the trait bound cannot make a preference out of, with a diagnostic at
/// the field.
///
/// The bound itself accepts any `PreferenceValue` type; this only catches the shapes that are
/// never one — references, tuples, arrays — before the trait error does.
fn ensure_value_type(ty: &Type, key: &Ident) -> syn::Result<()> {
    if matches!(ty, Type::Path(_)) {
        return Ok(());
    }

    Err(syn::Error::new(
        ty.span(),
        format!(
            "preference field `{key}` must be an enum implementing `PreferenceValue`, or an \
             `Option<…>` of one, found `{}`",
            quote!(#ty)
        ),
    ))
}

/// The field's description: its doc comment lines, trimmed and joined with single spaces.
fn description(field: &Field, key: &Ident) -> syn::Result<String> {
    let mut lines: Vec<String> = Vec::new();

    for attribute in &field.attrs {
        if !attribute.path().is_ident("doc") {
            continue;
        }

        let Meta::NameValue(name_value) = &attribute.meta else {
            continue;
        };
        let Expr::Lit(ExprLit {
            lit: Lit::Str(text),
            ..
        }) = &name_value.value
        else {
            continue;
        };

        lines.extend(
            text.value()
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(ToOwned::to_owned),
        );
    }

    if lines.is_empty() {
        return Err(syn::Error::new(
            key.span(),
            format!("preference field `{key}` needs a doc comment"),
        ));
    }

    Ok(lines.join(" "))
}

/// Expands `#[derive(PreferenceSet)]` for the parsed struct.
pub(super) fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new(
            input.ident.span(),
            "`PreferenceSet` can only be derived for a struct with named fields",
        ));
    };

    if !input.generics.params.is_empty() {
        return Err(syn::Error::new(
            input.generics.span(),
            "`PreferenceSet` does not support generic structs",
        ));
    }

    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new(
            data.fields.span(),
            "`PreferenceSet` requires named fields: the field's name is the preference key",
        ));
    };

    let preferences = fields
        .named
        .iter()
        .map(Preference::parse)
        .collect::<syn::Result<Vec<_>>>()?;

    let ident = &input.ident;
    let vis = &input.vis;
    let reader = format_ident!("{ident}Reader");

    let specs = preferences
        .iter()
        .map(|preference| preference.spec(&quote!(plugin)));
    let methods = preferences
        .iter()
        .map(|preference| preference.reader_method(vis));

    let reader_doc = LitStr::new(
        &format!("Reads the preferences of one sender as typed values, generated for `{ident}`."),
        ident.span(),
    );
    let constructor_doc = LitStr::new(
        &format!(
            "Creates a `{reader}`; `plugin` is the plugin's `Registered::NAME`, which keys the \
             stored values."
        ),
        ident.span(),
    );

    let field_reads = preferences.iter().map(|preference| &preference.key);

    Ok(quote! {
        impl ::zeta_plugin::accounts::PreferenceSet for #ident {
            fn preferences(
                plugin: &'static str,
            ) -> ::std::vec::Vec<::zeta_plugin::accounts::PreferenceSpec> {
                ::std::vec![#(#specs),*]
            }
        }

        impl #ident {
            /// References every declared field, which is what keeps the declaration struct's
            /// fields from reading as dead code: the derive consumes them through the
            /// descriptors and readers, never through a field access.
            #[allow(dead_code, reason = "the fields are declarations consumed by the derive")]
            fn __declared_fields(&self) {
                let _ = (#(&self.#field_reads),*);
            }

            #[doc = #constructor_doc]
            #[must_use]
            #vis const fn reader<'a>(
                service: &'a ::zeta_plugin::accounts::Preferences,
                plugin: &'static str,
                identity: ::core::option::Option<&'a ::zeta_plugin::accounts::UserIdentity>,
            ) -> #reader<'a> {
                #reader {
                    service,
                    plugin,
                    identity,
                }
            }
        }

        #[doc = #reader_doc]
        #[derive(Debug)]
        #vis struct #reader<'a> {
            service: &'a ::zeta_plugin::accounts::Preferences,
            plugin: &'static str,
            identity: ::core::option::Option<&'a ::zeta_plugin::accounts::UserIdentity>,
        }

        impl<'a> #reader<'a> {
            #(#methods)*
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The expansion of `source`, with the spacing between tokens removed.
    fn rendered(source: &str) -> String {
        let input: DeriveInput = syn::parse_str(source).expect("a parsable derive input");

        super::super::compact(&expand(&input).expect("the derive should expand"))
    }

    /// The error from expanding `source`.
    fn error(source: &str) -> syn::Error {
        let input: DeriveInput = syn::parse_str(source).expect("a parsable derive input");

        expand(&input).expect_err("the derive should be rejected")
    }

    /// A set with one required and one optional preference.
    const SET: &str = r"
        pub struct Prefs {
            /// Whether the bot summarizes every link.
            summary: Summary,
            /// The chat model to ask.
            model: Option<Model>,
        }
        ";

    #[test]
    fn every_field_becomes_a_descriptor() {
        let rendered = rendered(SET);

        assert!(
            rendered.contains("impl::zeta_plugin::accounts::PreferenceSetforPrefs"),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                "fnpreferences(plugin:&'staticstr,)->::std::vec::Vec<::zeta_plugin::accounts::PreferenceSpec>{::std::vec!["
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                r#"::zeta_plugin::accounts::PreferenceSpec{plugin:plugin,key:"summary",kind:::zeta_plugin::accounts::PreferenceKind::Enum(<Summaryas::zeta_plugin::accounts::PreferenceValue>::VALUES),description:"Whether the bot summarizes every link.",default:::core::option::Option::Some(<Summaryas::zeta_plugin::accounts::PreferenceValue>::as_str(<Summaryas::core::default::Default>::default())),}"#
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                r#"::zeta_plugin::accounts::PreferenceSpec{plugin:plugin,key:"model",kind:::zeta_plugin::accounts::PreferenceKind::Enum(<Modelas::zeta_plugin::accounts::PreferenceValue>::VALUES),description:"The chat model to ask.",default:::core::option::Option::None,}"#
            ),
            "{rendered}"
        );
    }

    #[test]
    fn the_reader_struct_and_its_constructor_are_generated() {
        let rendered = rendered(SET);

        assert!(
            rendered.contains(
                "pubstructPrefsReader<'a>{service:&'a::zeta_plugin::accounts::Preferences,\
                 plugin:&'staticstr,\
                 identity:::core::option::Option<&'a::zeta_plugin::accounts::UserIdentity>,}"
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                "pubconstfnreader<'a>(service:&'a::zeta_plugin::accounts::Preferences,\
                 plugin:&'staticstr,\
                 identity:::core::option::Option<&'a::zeta_plugin::accounts::UserIdentity>,)\
                 ->PrefsReader<'a>{PrefsReader{service,plugin,identity,}}"
            ),
            "{rendered}"
        );
        assert!(rendered.contains("impl<'a>PrefsReader<'a>"), "{rendered}");
    }

    #[test]
    fn required_fields_read_through_value_or_default() {
        let rendered = rendered(SET);

        assert!(
            rendered.contains(
                "pubasyncfnsummary(&self,)->::core::result::Result<Summary,\
                 ::zeta_plugin::database::DbError>{"
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains("self.service.value_or_default(identity,&spec).await?;"),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                "let::core::option::Option::Some(identity)=self.identityelse{\
                 return::core::result::Result::Ok(<Summaryas\
                 ::core::default::Default>::default(),);};"
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                "value.and_then(<Summaryas\
                 ::zeta_plugin::accounts::PreferenceValue>::canonical).unwrap_or_default()"
            ),
            "{rendered}"
        );
    }

    #[test]
    fn optional_fields_read_through_value() {
        let rendered = rendered(SET);

        assert!(
            rendered.contains(
                "pubasyncfnmodel(&self,)->::core::result::Result<::core::option::Option<Model>,\
                 ::zeta_plugin::database::DbError>{"
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains("letvalue=self.service.value(identity,&spec).await?;"),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                "::core::result::Result::Ok(value.and_then(<Modelas\
                 ::zeta_plugin::accounts::PreferenceValue>::canonical,))"
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains("return::core::result::Result::Ok(::core::option::Option::None);};"),
            "{rendered}"
        );
    }

    #[test]
    fn the_reader_methods_are_as_visible_as_the_set() {
        let public = rendered("pub struct Prefs { /// A summary.\n summary: Summary, }");

        assert!(public.contains("pubasyncfnsummary(&self,)"), "{public}");

        let private = rendered("struct Prefs { /// A summary.\n summary: Summary, }");
        assert!(!private.contains("pubasyncfnsummary"), "{private}");
    }

    #[test]
    fn doc_comment_lines_are_trimmed_and_joined() {
        let rendered = rendered(
            r"
            struct Prefs {
                ///   Whether the bot
                ///
                /// summarizes every link.
                summary: Summary,
            }
            ",
        );

        assert!(
            rendered.contains(r#"description:"Whether the bot summarizes every link.""#),
            "{rendered}"
        );
    }

    #[test]
    fn an_undocumented_field_is_rejected() {
        let error = error("struct Prefs { summary: Summary }");

        assert!(
            error
                .to_string()
                .contains("preference field `summary` needs a doc comment"),
            "{error}"
        );
    }

    #[test]
    fn enums_and_unions_are_rejected() {
        let error = error("enum Prefs { Summary }");

        assert!(
            error
                .to_string()
                .contains("can only be derived for a struct"),
            "{error}"
        );
    }

    #[test]
    fn generic_structs_are_rejected() {
        let error = error("struct Prefs<T> { /// A summary.\n summary: T }");

        assert!(error.to_string().contains("generic"), "{error}");
    }

    #[test]
    fn tuple_structs_are_rejected() {
        let error = error("struct Prefs(Summary);");

        assert!(error.to_string().contains("named fields"), "{error}");
    }

    #[test]
    fn nested_options_are_rejected() {
        let error = error("struct Prefs { /// A summary.\n summary: Option<Option<Summary>> }");

        assert!(
            error
                .to_string()
                .contains("`Option<Option<…>>` is not supported"),
            "{error}"
        );
    }

    #[test]
    fn options_without_a_value_type_are_rejected() {
        let bare = error("struct Prefs { /// A summary.\n summary: Option }");
        assert!(
            bare.to_string()
                .contains("`Option` needs a preference value type"),
            "{bare}"
        );

        let extra = error("struct Prefs { /// A summary.\n summary: Option<Summary, Model> }");
        assert!(
            extra
                .to_string()
                .contains("`Option` takes exactly one preference value type"),
            "{extra}"
        );
    }

    #[test]
    fn borrowed_fields_are_rejected() {
        let error = error("struct Prefs { /// A name.\n name: &'static str }");

        assert!(
            error.to_string().contains(
                "must be an enum implementing `PreferenceValue`, or an `Option<…>` of one"
            ),
            "{error}"
        );
    }

    #[test]
    fn field_attributes_are_rejected() {
        let error =
            error("struct Prefs { /// A summary.\n #[preference(name = \"s\")] summary: Summary }");

        assert!(
            error
                .to_string()
                .contains("is not supported on the preference field `summary`"),
            "{error}"
        );
    }
}
