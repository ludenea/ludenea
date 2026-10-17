//! Derive macros for `ludenea_ecs`, re-exported by `ludenea_ecs` and the `ludenea` facade.

use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::{
    Data, DeriveInput, Fields, GenericArgument, Ident, Lifetime, LitStr, PathArguments, Type,
    parse_macro_input,
};

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

/// Implements `QueryData` for a struct with one lifetime whose fields are `&'w T`,
/// `&'w mut T`, `Entity`, or `Option` of those, and `ReadOnlyQueryData` when no field is
/// `&mut`. An optional `#[query(filter = F)]` restricts it to entities passing `F`.
#[proc_macro_derive(Query, attributes(query))]
pub fn derive_query(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand_query(&input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.into_compile_error().into(),
    }
}

fn expand_query(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let ecs = ecs_path();
    let ident = &input.ident;
    let lifetime = match input.generics.lifetimes().collect::<Vec<_>>()[..] {
        [param] if input.generics.params.len() == 1 => &param.lifetime,
        _ => {
            return Err(syn::Error::new_spanned(
                &input.generics,
                "a query struct takes exactly one lifetime parameter, such as `'w`",
            ));
        }
    };
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    ident,
                    "a query struct needs named fields",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                ident,
                "only structs can derive `Query`",
            ));
        }
    };
    if fields.is_empty() || fields.len() > 8 {
        return Err(syn::Error::new_spanned(
            ident,
            "a query struct has between 1 and 8 fields",
        ));
    }
    let names: Vec<_> = fields.iter().map(|field| &field.ident).collect();
    let types: Vec<_> = fields.iter().map(|field| &field.ty).collect();
    let filter = query_filter(input)?.map_or_else(|| quote!(()), |filter| quote!(#filter));
    let data = quote!((#(#types,)*));

    let mut writes = false;
    for ty in &types {
        writes |= field_writes(ty, lifetime)?;
    }
    let read_only = if writes {
        quote!()
    } else {
        quote! {
            unsafe impl<#lifetime> #ecs::ReadOnlyQueryData for #ident<#lifetime> {
                type ReadItem<'__item> = #ident<'__item>;

                fn into_read(item: Self::Item<'_>) -> Self::ReadItem<'_> {
                    // Concrete calls, not `where` bounds: they catch an alias of `&mut T`
                    // or of `Entity` that the syntactic check above cannot see.
                    #(#ecs::query::__assert_read_only::<#types>();)*
                    item
                }
            }
        }
    };

    Ok(quote! {
        impl<#lifetime> #ecs::QueryData for #ident<#lifetime> {
            type Item<'__item> = #ident<'__item>;
            type State<'__world> = (
                <#data as #ecs::QueryData>::State<'__world>,
                <#filter as #ecs::QueryFilter>::State<'__world>,
            );

            fn accesses(accesses: &mut ::std::vec::Vec<#ecs::Access>) {
                <#data as #ecs::QueryData>::accesses(accesses);
                <#filter as #ecs::QueryFilter>::accesses(accesses);
            }

            fn fetch(
                world: &#ecs::World,
            ) -> ::core::result::Result<Self::State<'_>, #ecs::EcsError> {
                ::core::result::Result::Ok((
                    <#data as #ecs::QueryData>::fetch(world)?,
                    <#filter as #ecs::QueryFilter>::fetch(world)?,
                ))
            }

            fn matches(state: &Self::State<'_>, entity: #ecs::Entity) -> bool {
                <#data as #ecs::QueryData>::matches(&state.0, entity)
                    && <#filter as #ecs::QueryFilter>::matches(&state.1, entity)
            }

            unsafe fn get<'__item>(
                state: &'__item Self::State<'_>,
                entity: #ecs::Entity,
            ) -> Self::Item<'__item> {
                let (#(#names,)*) = unsafe { <#data as #ecs::QueryData>::get(&state.0, entity) };
                #ident { #(#names,)* }
            }
        }

        #read_only
    })
}

fn query_filter(input: &DeriveInput) -> syn::Result<Option<Type>> {
    let mut filter = None;
    for attr in input
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("query"))
    {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("filter") {
                filter = Some(meta.value()?.parse::<Type>()?);
                Ok(())
            } else {
                Err(meta.error("expected `filter = ...`"))
            }
        })?;
    }
    Ok(filter)
}

/// Checks that a field type is `&'w T`, `&'w mut T`, `Entity`, `Option<&'w T>`, or
/// `Option<&'w mut T>`, and returns whether it writes. Anything else (tuples, aliases,
/// nested query structs) is rejected so read-only detection cannot be fooled.
fn field_writes(ty: &Type, lifetime: &Lifetime) -> syn::Result<bool> {
    let error = || {
        syn::Error::new_spanned(
            ty,
            format!(
                "a query field is `&{lifetime} T`, `&{lifetime} mut T`, `Entity`, \
                 `Option<&{lifetime} T>`, or `Option<&{lifetime} mut T>`"
            ),
        )
    };
    let reference_writes = |ty: &Type| match ty {
        Type::Reference(reference)
            if reference
                .lifetime
                .as_ref()
                .is_some_and(|used| used.ident == lifetime.ident) =>
        {
            Ok(reference.mutability.is_some())
        }
        _ => Err(error()),
    };
    match ty {
        Type::Reference(_) => reference_writes(ty),
        Type::Path(path) if path.qself.is_none() => {
            let segment = path.path.segments.last().ok_or_else(error)?;
            match (&segment.arguments, segment.ident.to_string().as_str()) {
                (PathArguments::None, "Entity") => Ok(false),
                (PathArguments::AngleBracketed(arguments), "Option") => {
                    match arguments.args.iter().collect::<Vec<_>>()[..] {
                        [GenericArgument::Type(inner)] => reference_writes(inner),
                        _ => Err(error()),
                    }
                }
                _ => Err(error()),
            }
        }
        _ => Err(error()),
    }
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
