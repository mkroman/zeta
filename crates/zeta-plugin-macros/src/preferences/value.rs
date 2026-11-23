//! The `#[derive(PreferenceValue)]` expansion.
//!
//! The enum's variants, in declaration order, become `PreferenceValue::VALUES`; the container's
//! `rename_all` and a variant's own `name` only change how each variant is spelled.

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::{Data, DeriveInput, Fields, Ident, LitStr, Token};

/// How the container's `rename_all` spells every variant.
#[derive(Clone, Copy)]
enum RenameAll {
    /// `UPPER`: the variant ident uppercased.
    Upper,
    /// `lower`: the variant ident lowercased.
    Lower,
}

impl RenameAll {
    /// The spelling of one variant under this convention.
    fn spelling(self, variant: &Ident) -> String {
        let name = variant.to_string();

        match self {
            Self::Upper => name.to_uppercase(),
            Self::Lower => name.to_lowercase(),
        }
    }
}

/// The `#[preference(...)]` arguments of the enum itself.
#[derive(Default)]
struct ContainerArgs {
    /// The spelling convention applied to every variant, from `rename_all = "…"`.
    rename_all: Option<RenameAll>,
}

impl Parse for ContainerArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut rename_all: Option<RenameAll> = None;

        while !input.is_empty() {
            let ident: Ident = input.parse()?;

            match ident.to_string().as_str() {
                "rename_all" => {
                    input.parse::<Token![=]>()?;
                    let value: LitStr = input.parse()?;
                    let convention = match value.value().as_str() {
                        "UPPER" => RenameAll::Upper,
                        "lower" => RenameAll::Lower,
                        other => {
                            return Err(syn::Error::new(
                                value.span(),
                                format!(
                                    "unsupported `rename_all` value `{other}`; expected `UPPER` \
                                     or `lower`"
                                ),
                            ));
                        }
                    };

                    if rename_all.replace(convention).is_some() {
                        return Err(syn::Error::new(
                            ident.span(),
                            "duplicate `rename_all` argument",
                        ));
                    }
                }
                _ => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!("unknown preference attribute `{ident}`; expected `rename_all`"),
                    ));
                }
            }

            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(Self { rename_all })
    }
}

/// The `#[preference(...)]` arguments of one variant.
#[derive(Default)]
struct VariantArgs {
    /// The variant's own spelling, from `name = "…"`.
    name: Option<LitStr>,
}

impl Parse for VariantArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut name: Option<LitStr> = None;

        while !input.is_empty() {
            let ident: Ident = input.parse()?;

            match ident.to_string().as_str() {
                "name" => {
                    input.parse::<Token![=]>()?;
                    let value: LitStr = input.parse()?;

                    if name.replace(value).is_some() {
                        return Err(syn::Error::new(ident.span(), "duplicate `name` argument"));
                    }
                }
                _ => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!("unknown preference attribute `{ident}`; expected `name`"),
                    ));
                }
            }

            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(Self { name })
    }
}

/// Reads the container's `#[preference(...)]` arguments.
fn container_args(input: &DeriveInput) -> syn::Result<ContainerArgs> {
    super::preference_attribute(&input.attrs)?
        .map_or_else(|| Ok(ContainerArgs::default()), syn::Attribute::parse_args)
}

/// Reads one variant's `#[preference(...)]` arguments.
fn variant_args(variant: &syn::Variant) -> syn::Result<VariantArgs> {
    super::preference_attribute(&variant.attrs)?
        .map_or_else(|| Ok(VariantArgs::default()), syn::Attribute::parse_args)
}

/// Expands `#[derive(PreferenceValue)]` for the parsed enum.
pub(super) fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let Data::Enum(data) = &input.data else {
        return Err(syn::Error::new(
            input.ident.span(),
            "`PreferenceValue` can only be derived for an enum",
        ));
    };

    if !input.generics.params.is_empty() {
        return Err(syn::Error::new(
            input.generics.span(),
            "`PreferenceValue` does not support generic enums",
        ));
    }

    let container = container_args(input)?;
    let mut variants: Vec<&Ident> = Vec::with_capacity(data.variants.len());
    let mut spellings: Vec<LitStr> = Vec::with_capacity(data.variants.len());

    for variant in &data.variants {
        if !matches!(variant.fields, Fields::Unit) {
            return Err(syn::Error::new(
                variant.fields.span(),
                "a preference variant cannot have fields: the value is one of a fixed set of \
                 spellings",
            ));
        }

        let args = variant_args(variant)?;
        let spelling = args.name.unwrap_or_else(|| {
            let convention = container.rename_all;

            LitStr::new(
                &convention.map_or_else(
                    || variant.ident.to_string(),
                    |convention| convention.spelling(&variant.ident),
                ),
                variant.ident.span(),
            )
        });

        variants.push(&variant.ident);
        spellings.push(spelling);
    }

    let ident = &input.ident;
    let canonical: Vec<TokenStream> = variants
        .iter()
        .zip(&spellings)
        .map(|(variant, spelling)| {
            quote! {
                if value.eq_ignore_ascii_case(#spelling) {
                    return ::core::option::Option::Some(Self::#variant);
                }
            }
        })
        .collect();
    let rendered: Vec<TokenStream> = variants
        .iter()
        .zip(&spellings)
        .map(|(variant, spelling)| quote!(Self::#variant => #spelling))
        .collect();

    Ok(quote! {
        impl ::zeta_plugin::accounts::PreferenceValue for #ident {
            const VALUES: &'static [&'static str] = &[#(#spellings),*];

            fn canonical(value: &str) -> ::core::option::Option<Self> {
                #(#canonical)*

                ::core::option::Option::None
            }

            fn as_str(self) -> &'static str {
                match self {
                    #(#rendered),*
                }
            }
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

    #[test]
    fn variants_keep_their_written_spelling_by_default() {
        let rendered = rendered("enum Accent { Danish, German }");

        assert!(
            rendered.contains(r#"constVALUES:&'static[&'staticstr]=&["Danish","German"];"#),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                r#"ifvalue.eq_ignore_ascii_case("Danish"){return::core::option::Option::Some(Self::Danish);}"#
            ),
            "{rendered}"
        );
    }

    #[test]
    fn rename_all_uppercases_the_variants() {
        let rendered =
            rendered("#[preference(rename_all = \"UPPER\")] enum Accent { Danish, German }");

        assert!(rendered.contains(r#"&["DANISH","GERMAN"]"#), "{rendered}");
    }

    #[test]
    fn rename_all_lowercases_the_variants() {
        let rendered =
            rendered("#[preference(rename_all = \"lower\")] enum Accent { Danish, German }");

        assert!(rendered.contains(r#"&["danish","german"]"#), "{rendered}");
    }

    #[test]
    fn a_variant_name_overrides_the_container_convention() {
        let rendered = rendered(
            "#[preference(rename_all = \"UPPER\")] enum Accent { \
             #[preference(name = \"de\")] German, French }",
        );

        assert!(rendered.contains(r#"&["de","FRENCH"]"#), "{rendered}");
    }

    #[test]
    fn as_str_renders_every_canonical_spelling() {
        let rendered = rendered("enum Accent { Danish, German }");

        assert!(
            rendered.contains(r#"matchself{Self::Danish=>"Danish",Self::German=>"German"}"#),
            "{rendered}"
        );
    }

    #[test]
    fn structs_are_rejected() {
        let error = error("struct NotAnEnum;");

        assert!(
            error
                .to_string()
                .contains("can only be derived for an enum"),
            "{error}"
        );
    }

    #[test]
    fn generic_enums_are_rejected() {
        let error = error("enum Generic<T> { Value(T) }");

        assert!(error.to_string().contains("generic"), "{error}");
    }

    #[test]
    fn variants_with_fields_are_rejected() {
        let error = error("enum Accent { Danish(u8) }");

        assert!(error.to_string().contains("cannot have fields"), "{error}");
    }

    #[test]
    fn unsupported_rename_all_values_are_rejected() {
        let error = error("#[preference(rename_all = \"snake_case\")] enum Accent { Danish }");

        assert!(
            error
                .to_string()
                .contains("unsupported `rename_all` value `snake_case`"),
            "{error}"
        );
    }

    #[test]
    fn unknown_attributes_are_rejected() {
        let container = error("#[preference(spelling = \"lower\")] enum Accent { Danish }");
        assert!(
            container
                .to_string()
                .contains("unknown preference attribute `spelling`"),
            "{container}"
        );

        let variant = error("enum Accent { #[preference(rename = \"de\")] Danish }");
        assert!(
            variant
                .to_string()
                .contains("unknown preference attribute `rename`"),
            "{variant}"
        );
    }

    #[test]
    fn duplicate_arguments_are_rejected() {
        let duplicate =
            error("#[preference(rename_all = \"UPPER\", rename_all = \"lower\")] enum A { B }");
        assert!(
            duplicate.to_string().contains("duplicate `rename_all`"),
            "{duplicate}"
        );

        let variant = error("enum A { #[preference(name = \"a\", name = \"b\")] B }");
        assert!(
            variant.to_string().contains("duplicate `name`"),
            "{variant}"
        );

        let attribute = error("#[preference(rename_all = \"UPPER\")] #[preference] enum A { B }");
        assert!(
            attribute
                .to_string()
                .contains("duplicate `#[preference(...)]` attribute"),
            "{attribute}"
        );
    }
}
