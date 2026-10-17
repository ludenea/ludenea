//! Derive macros for `ludenea_ecs`, re-exported by `ludenea_ecs` and the `ludenea` facade.

use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::{DeriveInput, Ident, LitStr, parse_macro_input};

/// Implements `Component` with a stable name: `module_path!()::Type` by default,
/// or the value of `#[component(name = "...")]`.
#[proc_macro_derive(Component, attributes(component))]
pub fn derive_component(input: TokenStream) -> TokenStream {
    derive_named(input, "Component", "component")
}

/// Implements `Resource` with a stable name: `module_path!()::Type` by default,
/// or the value of `#[resource(name = "...")]`.
#[proc_macro_derive(Resource, attributes(resource))]
pub fn derive_resource(input: TokenStream) -> TokenStream {
    derive_named(input, "Resource", "resource")
}

fn derive_named(input: TokenStream, trait_name: &str, attribute: &str) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(&input, trait_name, attribute) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.into_compile_error().into(),
    }
}

fn expand(input: &DeriveInput, trait_name: &str, attribute: &str) -> syn::Result<TokenStream2> {
    let ecs = ecs_path();
    let trait_ident = Ident::new(trait_name, Span::call_site());
    let ident = &input.ident;
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
    let name = match name_override(input, attribute)? {
        Some(name) => quote!(#name),
        None => {
            let type_name = ident.to_string();
            quote!(::core::concat!(::core::module_path!(), "::", #type_name))
        }
    };
    Ok(quote! {
        impl #impl_generics #ecs::#trait_ident for #ident #type_generics #where_clause {
            const NAME: &'static str = #name;
        }
    })
}

fn name_override(input: &DeriveInput, attribute: &str) -> syn::Result<Option<LitStr>> {
    let mut name = None;
    for attr in input
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident(attribute))
    {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                name = Some(meta.value()?.parse::<LitStr>()?);
                Ok(())
            } else {
                Err(meta.error("expected `name = \"...\"`"))
            }
        })?;
    }
    Ok(name)
}

/// Resolves `ludenea_ecs` directly when it is a dependency, otherwise through the facade.
fn ecs_path() -> TokenStream2 {
    match crate_name("ludenea-ecs") {
        Ok(FoundCrate::Itself) => quote!(::ludenea_ecs),
        Ok(FoundCrate::Name(name)) => {
            let name = Ident::new(&name, Span::call_site());
            quote!(::#name)
        }
        Err(_) => match crate_name("ludenea") {
            Ok(FoundCrate::Name(name)) => {
                let name = Ident::new(&name, Span::call_site());
                quote!(::#name::ecs)
            }
            _ => quote!(::ludenea::ecs),
        },
    }
}
