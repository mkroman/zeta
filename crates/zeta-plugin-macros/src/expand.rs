//! Classification, validation, and code generation for the [`plugin`](crate::plugin) macro.
//!
//! The macro is applied to an inherent impl block. Classification reads the `#[command]` and
//! `#[handler]` annotations off the block's methods and validates them, rendering then emits
//! the `Plugin` implementation next to the stripped inherent impl. Every generated path is
//! absolute (`::zeta_plugin::…`), because a proc macro has no `$crate`.
//!
//! The stages are separate modules: `model` holds the classified plugin, `classify` reads an
//! impl block into it, `validate` enforces the signature and cross-method rules, and `render`
//! turns it back into tokens. `expand` runs them in order, stopping at the first error so a
//! plugin author sees one diagnostic at a time.

// `pub` would not widen where these items are reachable — the module tree is private — so the
// restricted visibilities only document which submodule owns each item. Clippy's nursery lint
// sees the private module but not that intent, so it is silenced for the tree.
#![allow(clippy::redundant_pub_crate)]

mod classify;
mod model;
mod render;
mod validate;

#[cfg(test)]
mod tests;

use proc_macro2::TokenStream;

use crate::args::PluginArgs;

use self::classify::{classify, validate_handler_subscriptions};
use self::render::{render, strip_annotations};

#[cfg(test)]
pub(crate) use self::validate::spec_const_name;

#[cfg(test)]
use self::{
    model::{ConstructorParameter, Plugin},
    render::dispatch_arm,
    validate::validate_command,
};

/// Classifies, validates, and renders the macro output for one `#[plugin]` impl block.
pub(crate) fn expand(args: &PluginArgs, mut inherent: syn::ItemImpl) -> TokenStream {
    match classify(args, &inherent) {
        Ok(plugin) => {
            if let Some(error) = validate_handler_subscriptions(&plugin) {
                return error.to_compile_error();
            }

            strip_annotations(&plugin, &mut inherent);

            render(&plugin, &inherent)
        }
        Err(error) => error.to_compile_error(),
    }
}
