//! The derives that declare a plugin's per-user preferences.
//!
//! `#[derive(PreferenceValue)]` turns a preference enum into the list of spellings it accepts
//! and a case-insensitive parser; `#[derive(PreferenceSet)]` turns a struct of preference fields
//! into the descriptors the runtime publishes and the typed reader a plugin reads the values
//! through.
//!
//! Every generated path is absolute (`::zeta_plugin::…`, `::core::…`, `::std::…`), because a
//! proc macro has no `$crate`.

mod set;
mod value;

use proc_macro2::TokenStream;
use syn::spanned::Spanned;
use syn::{Attribute, DeriveInput};

use self::set::expand as expand_set;
use self::value::expand as expand_value;

/// Expands `#[derive(PreferenceValue)]`, rendering the first error as `compile_error!`.
pub(crate) fn preference_value(input: &DeriveInput) -> TokenStream {
    expand_value(input).unwrap_or_else(syn::Error::into_compile_error)
}

/// Expands `#[derive(PreferenceSet)]`, rendering the first error as `compile_error!`.
pub(crate) fn preference_set(input: &DeriveInput) -> TokenStream {
    expand_set(input).unwrap_or_else(syn::Error::into_compile_error)
}

/// The parsed `#[preference(...)]` attribute of one item, rejecting a repetition.
///
/// The attribute is a derive helper: it is accepted on any item the two derives inspect, and
/// each derived item reads only the options its own grammar defines.
fn preference_attribute(attributes: &[Attribute]) -> syn::Result<Option<&Attribute>> {
    let mut found: Option<&Attribute> = None;

    for attribute in attributes {
        if !attribute.path().is_ident("preference") {
            continue;
        }

        if found.is_some() {
            return Err(syn::Error::new(
                attribute.span(),
                "duplicate `#[preference(...)]` attribute",
            ));
        }

        found = Some(attribute);
    }

    Ok(found)
}

/// The expanded tokens with the spacing between them removed, for the assertion tests.
///
/// Whitespace inside string literals is preserved, so a generated description can be asserted
/// verbatim.
#[cfg(test)]
fn compact(tokens: &TokenStream) -> String {
    let source = tokens.to_string();
    let mut compacted = String::with_capacity(source.len());
    let mut in_string = false;
    let mut escaped = false;

    for character in source.chars() {
        if in_string {
            compacted.push(character);

            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }

            continue;
        }

        if character == '"' {
            in_string = true;
            compacted.push(character);
        } else if !character.is_whitespace() {
            compacted.push(character);
        }
    }

    compacted
}
