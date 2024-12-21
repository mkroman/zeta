extern crate proc_macro;

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{parse_macro_input, punctuated::Punctuated, DeriveInput, Token};

#[proc_macro_derive(Plugin, attributes(plugin))]
pub fn derive_plugin(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let input_attrs = input
        .attrs
        .iter()
        .filter(|x| x.path().is_ident("plugin"))
        .collect::<Vec<&syn::Attribute>>();
    let name = input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    println!("{impl_generics:?}");

    // for attr in input_attrs {
    //     let args = attr.parse_args_with(Punctuated::<PluginArg, Token![,]>::parse_terminated);

    //     println!("args: {args:?}");
    // }

    match input.data {
        syn::Data::Struct(_data_struct) => {
            let expanded = quote! {
                impl #impl_generics crate::plugin::Plugin for #name #ty_generics #where_clause {
                    fn name() -> Name {
                        Name(stringify!(#name))
                    }

                    fn author() -> Author {
                        Author("Benjiman Endicott <be@example.com>")
                    }

                    fn version() -> Version {
                        Version("0.1")
                    }
                }
            };

            expanded.into()
        }
        _ => todo!(),
    }
}
