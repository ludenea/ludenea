//! Derive macros for `ludenea_ecs`, re-exported by `ludenea_ecs` and the `ludenea` facade.

use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::ext::IdentExt;
use syn::{
    Attribute, Data, DeriveInput, Expr, Fields, GenericArgument, Ident, Lifetime, LitStr,
    PathArguments, Token, Type, parenthesized, parse_macro_input, parse_quote, token,
};

/// Implements `Component` with a stable name: `module_path!()::Type` by default,
/// or the value of `#[component(name = "...")]`. Also implements `Schema`.
#[proc_macro_derive(Component, attributes(component))]
pub fn derive_component(input: TokenStream) -> TokenStream {
    derive_named(input, "Component", "component")
}

/// Implements `Resource` with a stable name: `module_path!()::Type` by default,
/// or the value of `#[resource(name = "...")]`. Also implements `Schema`.
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
    let schema = expand_schema(input)?;
    Ok(quote! {
        impl #impl_generics #ecs::#trait_ident for #ident #type_generics #where_clause {
            const NAME: &'static str = #name;
        }

        #schema
    })
}

/// Implements `Schema` for a struct (named, tuple, or unit) or an enum, naming it
/// `module_path!()::Type`. Field types must implement `Schema`.
///
/// The schema must match the serde shape, so serde attributes that change it (`rename`,
/// `rename_all`, `tag`, `content`, `untagged`, `flatten`, `skip`, `skip_serializing`,
/// `skip_deserializing`, `default`, `with`) fail to compile. The container attributes
/// `try_from`, `from`, and `into` are accepted: the mirror type must have identical fields.
#[proc_macro_derive(Schema)]
pub fn derive_schema(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand_schema(&input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.into_compile_error().into(),
    }
}

fn expand_schema(input: &DeriveInput) -> syn::Result<TokenStream2> {
    check_serde_attributes(&input.attrs)?;
    let schema = schema_path();
    let ident = &input.ident;
    let name = ident.to_string();
    let name = quote!(::std::string::String::from(
        ::core::concat!(::core::module_path!(), "::", #name)
    ));
    let named_fields = |fields: &syn::punctuated::Punctuated<syn::Field, Token![,]>| {
        fields
            .iter()
            .map(|field| {
                check_serde_attributes(&field.attrs)?;
                let field_name = field
                    .ident
                    .as_ref()
                    .expect("named field")
                    .unraw()
                    .to_string();
                let ty = &field.ty;
                Ok(quote! {
                    #schema::FieldSchema {
                        name: ::std::string::String::from(#field_name),
                        schema: <#ty as #schema::Schema>::schema(),
                    }
                })
            })
            .collect::<syn::Result<Vec<_>>>()
    };
    let unnamed_types = |fields: &syn::punctuated::Punctuated<syn::Field, Token![,]>| {
        fields
            .iter()
            .map(|field| {
                check_serde_attributes(&field.attrs)?;
                let ty = &field.ty;
                Ok(quote!(<#ty as #schema::Schema>::schema()))
            })
            .collect::<syn::Result<Vec<_>>>()
    };
    let body = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => {
                let fields = named_fields(&fields.named)?;
                quote!(#schema::TypeSchema::Struct { name: #name, fields: ::std::vec![#(#fields),*] })
            }
            Fields::Unnamed(fields) => {
                let mut items = unnamed_types(&fields.unnamed)?;
                if items.len() == 1 {
                    let item = items.remove(0);
                    quote!(#schema::TypeSchema::Newtype {
                        name: #name,
                        item: ::std::boxed::Box::new(#item),
                    })
                } else {
                    quote!(#schema::TypeSchema::TupleStruct { name: #name, items: ::std::vec![#(#items),*] })
                }
            }
            Fields::Unit => quote!(#schema::TypeSchema::UnitStruct { name: #name }),
        },
        Data::Enum(data) => {
            let mut variants = Vec::new();
            for variant in &data.variants {
                check_serde_attributes(&variant.attrs)?;
                let variant_name = variant.ident.unraw().to_string();
                let fields = match &variant.fields {
                    Fields::Unit => quote!(#schema::VariantFields::Unit),
                    Fields::Unnamed(fields) => {
                        let mut items = unnamed_types(&fields.unnamed)?;
                        if items.len() == 1 {
                            let item = items.remove(0);
                            quote!(#schema::VariantFields::Newtype {
                                item: ::std::boxed::Box::new(#item),
                            })
                        } else {
                            quote!(#schema::VariantFields::Tuple { items: ::std::vec![#(#items),*] })
                        }
                    }
                    Fields::Named(fields) => {
                        let fields = named_fields(&fields.named)?;
                        quote!(#schema::VariantFields::Struct { fields: ::std::vec![#(#fields),*] })
                    }
                };
                variants.push(quote! {
                    #schema::VariantSchema {
                        name: ::std::string::String::from(#variant_name),
                        fields: #fields,
                    }
                });
            }
            quote!(#schema::TypeSchema::Enum { name: #name, variants: ::std::vec![#(#variants),*] })
        }
        Data::Union(_) => {
            return Err(syn::Error::new_spanned(
                ident,
                "only structs and enums can derive `Schema`",
            ));
        }
    };
    let mut generics = input.generics.clone();
    for param in generics.type_params_mut() {
        param.bounds.push(parse_quote!(#schema::Schema));
    }
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics #schema::Schema for #ident #type_generics #where_clause {
            fn schema() -> #schema::TypeSchema {
                #body
            }
        }
    })
}

/// Serde attributes that change the serialized shape, which a schema cannot follow.
const SHAPE_ATTRIBUTES: [&str; 11] = [
    "rename",
    "rename_all",
    "tag",
    "content",
    "untagged",
    "flatten",
    "skip",
    "skip_serializing",
    "skip_deserializing",
    "default",
    "with",
];

fn check_serde_attributes(attrs: &[Attribute]) -> syn::Result<()> {
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
        attr.parse_nested_meta(|meta| {
            if let Some(ident) = meta.path.get_ident()
                && SHAPE_ATTRIBUTES.contains(&ident.to_string().as_str())
            {
                return Err(meta.error(format!(
                    "`#[serde({ident})]` changes the serde shape, which `Schema` cannot follow"
                )));
            }
            // Consume the value of the attributes we accept (`try_from = "..."`, ...).
            if meta.input.peek(Token![=]) {
                meta.value()?.parse::<Expr>()?;
            } else if meta.input.peek(token::Paren) {
                let content;
                parenthesized!(content in meta.input);
                content.parse::<TokenStream2>()?;
            }
            Ok(())
        })?;
    }
    Ok(())
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
    crate_path("ludenea-ecs", "ludenea_ecs").unwrap_or_else(|| facade_path("ecs"))
}

/// Resolves `ludenea_schema` directly when it is a dependency, otherwise through
/// `ludenea_ecs::schema`, otherwise through the facade.
fn schema_path() -> TokenStream2 {
    crate_path("ludenea-schema", "ludenea_schema")
        .or_else(|| crate_path("ludenea-ecs", "ludenea_ecs").map(|ecs| quote!(#ecs::schema)))
        .unwrap_or_else(|| facade_path("schema"))
}

/// The path of the dependency `package`, if the crate being compiled has it (or is it).
fn crate_path(package: &str, own_name: &str) -> Option<TokenStream2> {
    match crate_name(package).ok()? {
        FoundCrate::Itself => {
            let name = Ident::new(own_name, Span::call_site());
            Some(quote!(::#name))
        }
        FoundCrate::Name(name) => {
            let name = Ident::new(&name, Span::call_site());
            Some(quote!(::#name))
        }
    }
}

fn facade_path(module: &str) -> TokenStream2 {
    let module = Ident::new(module, Span::call_site());
    match crate_path("ludenea", "ludenea") {
        Some(facade) => quote!(#facade::#module),
        None => quote!(::ludenea::#module),
    }
}
