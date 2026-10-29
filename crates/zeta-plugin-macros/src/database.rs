//! The `#[instrument_database]` attribute macro.
//!
//! Repository methods all carry the same `tracing::instrument` attribute: a span named after
//! the operation, the semantic-convention `db.*` fields every telemetry backend expects, and
//! `skip_all, err` so neither the connection pool nor the error's `Debug` output leaks into the
//! span. Writing it by hand is error-prone — one omitted field silently breaks the span — so the
//! attribute derives the whole block from the operation and collection.

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Ident, LitStr, Token};

/// The options accepted by `#[instrument_database(...)]`.
struct Arguments {
    /// The SQL verb, e.g. `SELECT`.
    operation: LitStr,
    /// The table the statement touches, e.g. `filters`.
    collection: LitStr,
    /// The span name and `db.query.summary`; defaults to `"<operation> <collection>"`.
    summary: Option<LitStr>,
}

impl Parse for Arguments {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut operation: Option<LitStr> = None;
        let mut collection: Option<LitStr> = None;
        let mut summary: Option<LitStr> = None;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let value: LitStr = input.parse()?;

            let slot = match key.to_string().as_str() {
                "operation" => &mut operation,
                "collection" => &mut collection,
                "summary" => &mut summary,
                _ => {
                    return Err(syn::Error::new(
                        key.span(),
                        "unknown `instrument_database` argument; expected `operation`, \
                         `collection`, or `summary`",
                    ));
                }
            };

            if slot.replace(value).is_some() {
                return Err(syn::Error::new(
                    key.span(),
                    format!("duplicate `{key}` argument"),
                ));
            }

            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(Self {
            operation: operation.ok_or_else(|| {
                syn::Error::new(
                    Span::call_site(),
                    "missing required `operation = \"…\"` argument",
                )
            })?,
            collection: collection.ok_or_else(|| {
                syn::Error::new(
                    Span::call_site(),
                    "missing required `collection = \"…\"` argument",
                )
            })?,
            summary,
        })
    }
}

/// Expands `#[instrument_database(...)]` on a repository method.
pub fn expand(arguments: TokenStream, item: TokenStream) -> TokenStream {
    let arguments = match syn::parse2::<Arguments>(arguments) {
        Ok(arguments) => arguments,
        Err(error) => return error.to_compile_error(),
    };

    let item = match syn::parse2::<syn::ItemFn>(item) {
        Ok(item) => item,
        Err(error) => return error.to_compile_error(),
    };

    let operation = arguments.operation;
    let collection = arguments.collection;
    let summary = arguments.summary.unwrap_or_else(|| {
        LitStr::new(
            &format!("{} {}", operation.value(), collection.value()),
            operation.span(),
        )
    });

    quote! {
        #[::tracing::instrument(
            name = #summary,
            skip_all,
            err,
            fields(
                db.system.name = "postgresql",
                db.namespace = %::zeta_plugin::database::namespace(&self.db),
                db.operation.name = #operation,
                db.collection.name = #collection,
                db.query.summary = #summary,
            )
        )]
        #item
    }
}

#[cfg(test)]
mod tests {
    use quote::quote;

    use super::*;

    #[test]
    fn derives_the_summary_from_the_operation_and_collection() {
        let expanded = expand(
            quote!(operation = "SELECT", collection = "filters"),
            quote!(
                async fn list(&self) -> Result<(), Error> {
                    Ok(())
                }
            ),
        )
        .to_string();

        assert!(expanded.contains(r#""SELECT filters""#), "{expanded}");
        assert!(expanded.contains(r#""SELECT""#), "{expanded}");
        assert!(expanded.contains(r#""filters""#), "{expanded}");
    }

    #[test]
    fn an_explicit_summary_names_statistics_queries() {
        let expanded = expand(
            quote!(
                operation = "SELECT",
                collection = "url_records",
                summary = "SELECT ofn statistics"
            ),
            quote!(
                async fn statistics(&self) -> Result<(), Error> {
                    Ok(())
                }
            ),
        )
        .to_string();

        assert!(
            expanded.contains(r#""SELECT ofn statistics""#),
            "{expanded}"
        );
        assert!(expanded.contains(r#""url_records""#), "{expanded}");
    }

    #[test]
    fn rejects_unknown_and_missing_arguments() {
        let unknown = expand(
            quote!(
                operation = "SELECT",
                collection = "filters",
                table = "filters"
            ),
            quote!(
                async fn list(&self) {}
            ),
        )
        .to_string();
        assert!(
            unknown.contains("unknown `instrument_database` argument"),
            "{unknown}"
        );

        let missing = expand(
            quote!(operation = "SELECT"),
            quote!(
                async fn list(&self) {}
            ),
        )
        .to_string();
        assert!(missing.contains("collection"), "{missing}");
    }
}
