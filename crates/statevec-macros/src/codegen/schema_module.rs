// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use quote::{format_ident, quote};
use syn::{
    Attribute, Ident, Item, ItemMod, LitStr, Meta, Token, parse::Parse, parse::ParseStream,
    punctuated::Punctuated, spanned::Spanned,
};

pub(crate) struct SchemaModuleArgs {
    pub version: LitStr,
}

impl Parse for SchemaModuleArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let key: Ident = input.parse()?;
        if key != Ident::new("version", key.span()) {
            return Err(syn::Error::new(key.span(), "expected `version = \"M.N\"`"));
        }
        input.parse::<Token![=]>()?;
        let version: LitStr = input.parse()?;
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        }
        if !input.is_empty() {
            return Err(syn::Error::new(
                input.span(),
                "unexpected trailing tokens in #[schema_module(...)]",
            ));
        }

        Ok(Self { version })
    }
}

pub(crate) fn expand_schema_module(
    args: SchemaModuleArgs,
    input: ItemMod,
) -> syn::Result<proc_macro2::TokenStream> {
    let (main, minor) = parse_schema_module_version(&args.version)?;
    let packed_version = u16::from_le_bytes([main, minor]);
    let attrs = input
        .attrs
        .into_iter()
        .filter(|attr| !attr.path().is_ident("schema_module"))
        .collect::<Vec<_>>();
    let vis = input.vis;
    let ident = input.ident;
    let Some((_, mut items)) = input.content else {
        return Err(syn::Error::new(
            ident.span(),
            "#[schema_module] only supports inline modules",
        ));
    };

    let mut enum_idents = Vec::<Ident>::new();
    let mut record_idents = Vec::<Ident>::new();
    let mut command_idents = Vec::<Ident>::new();
    let mut event_idents = Vec::<Ident>::new();

    for item in &mut items {
        match item {
            Item::Enum(item_enum) if has_enum_u8_derive(&item_enum.attrs) => {
                enum_idents.push(item_enum.ident.clone());
            }
            Item::Struct(item_struct) if has_schema_attr(&item_struct.attrs, "record") => {
                inject_schema_module_version(&mut item_struct.attrs, "record", packed_version)?;
                record_idents.push(item_struct.ident.clone());
            }
            Item::Struct(item_struct) if has_schema_attr(&item_struct.attrs, "command") => {
                inject_schema_module_version(&mut item_struct.attrs, "command", packed_version)?;
                command_idents.push(item_struct.ident.clone());
            }
            Item::Struct(item_struct) if has_schema_attr(&item_struct.attrs, "event") => {
                inject_schema_module_version(&mut item_struct.attrs, "event", packed_version)?;
                event_idents.push(item_struct.ident.clone());
            }
            _ => {}
        }
    }

    let enums = enum_idents.iter().map(|ident| {
        quote! { *<#ident as statevec_model::EnumU8>::definition() }
    });
    let records = record_idents.iter().map(|ident| {
        quote! { *<#ident as statevec_model::RecordSchema>::definition() }
    });
    let commands = command_idents.iter().map(|ident| {
        quote! { *<#ident as statevec_model::CommandSchema>::definition() }
    });
    let events = event_idents.iter().map(|ident| {
        quote! { *<#ident as statevec_model::EventSchema>::definition() }
    });
    let record_access_reexports = record_idents.iter().flat_map(|ident| {
        let access = format_ident!("{}Access", ident);
        let new_builder = format_ident!("New{}Builder", ident);
        let update_builder = format_ident!("Update{}Builder", ident);
        [
            quote! { pub use super::#access; },
            quote! { pub use super::#new_builder; },
            quote! { pub use super::#update_builder; },
        ]
    });
    let command_access_reexports = command_idents.iter().flat_map(|ident| {
        let access = format_ident!("{}Access", ident);
        let builder = format_ident!("{}Builder", ident);
        [
            quote! { pub use super::#access; },
            quote! { pub use super::#builder; },
        ]
    });
    let event_access_reexports = event_idents.iter().flat_map(|ident| {
        let access = format_ident!("{}Access", ident);
        let builder = format_ident!("{}Builder", ident);
        [
            quote! { pub use super::#access; },
            quote! { pub use super::#builder; },
        ]
    });

    Ok(quote! {
        #( #attrs )*
        #vis mod #ident {
            #( #items )*

            pub mod record {
                #( #record_access_reexports )*
            }

            pub mod command {
                #( #command_access_reexports )*
            }

            pub mod event {
                #( #event_access_reexports )*
            }

            pub const SCHEMA_VERSION: statevec_model::Version =
                statevec_model::Version::new(#main, #minor);

            pub fn registry() -> statevec_model::SchemaRegistry {
                statevec_model::SchemaRegistry::new(
                    SCHEMA_VERSION,
                    &[
                        #( #records, )*
                    ],
                    &[
                        #( #commands, )*
                    ],
                    &[
                        #( #events, )*
                    ],
                    &[
                        #( #enums, )*
                    ],
                )
            }

            pub fn idl_json() -> ::std::string::String {
                registry().to_idl_json()
            }

            pub fn schema_identity() -> statevec_model::SchemaIdentity {
                registry().identity()
            }
        }
    })
}

pub(crate) fn parse_schema_module_version(version: &LitStr) -> syn::Result<(u8, u8)> {
    let raw = version.value();
    let (main, minor) = raw.split_once('.').ok_or_else(|| {
        syn::Error::new(
            version.span(),
            "schema_module version must look like \"M.N\"",
        )
    })?;
    let main = main.parse::<u16>().map_err(|_| {
        syn::Error::new(
            version.span(),
            "schema_module main version must be an integer",
        )
    })?;
    let minor = minor.parse::<u16>().map_err(|_| {
        syn::Error::new(
            version.span(),
            "schema_module minor version must be an integer",
        )
    })?;
    if main > u8::MAX as u16 || minor > u8::MAX as u16 {
        return Err(syn::Error::new(
            version.span(),
            "schema_module version components must fit in u8",
        ));
    }
    Ok((main as u8, minor as u8))
}

pub(crate) fn has_schema_attr(attrs: &[Attribute], name: &str) -> bool {
    attrs.iter().any(|attr| attr.path().is_ident(name))
}

pub(crate) fn has_enum_u8_derive(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("derive") {
            return false;
        }

        let mut found = false;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("EnumU8") {
                found = true;
            }
            Ok(())
        });
        found
    })
}

pub(crate) fn attr_has_version_arg(attr: &Attribute) -> syn::Result<bool> {
    let syn::Meta::List(list) = &attr.meta else {
        return Ok(false);
    };

    let args = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
    for meta in args {
        let path = match meta {
            Meta::Path(path) => path,
            Meta::NameValue(nv) => nv.path,
            Meta::List(list) => list.path,
        };
        if path.is_ident("version") || path.is_ident("__schema_module_version") {
            return Ok(true);
        }
    }

    Ok(false)
}

#[cfg(test)]
#[path = "schema_module/ut_schema_module.rs"]
mod ut_schema_module;

pub(crate) fn inject_schema_module_version(
    attrs: &mut [Attribute],
    attr_name: &str,
    version: u16,
) -> syn::Result<()> {
    for attr in attrs.iter_mut() {
        if !attr.path().is_ident(attr_name) {
            continue;
        }
        if attr_has_version_arg(attr)? {
            return Err(syn::Error::new(
                attr.span(),
                "individual version is not allowed inside #[schema_module]; the module owns versioning",
            ));
        }

        let path = attr.path().clone();
        let syn::Meta::List(list) = &attr.meta else {
            return Err(syn::Error::new(
                attr.span(),
                "schema item attribute must use list syntax",
            ));
        };
        let tokens = list.tokens.clone();
        attr.meta = if tokens.is_empty() {
            syn::parse_quote!(#path(__schema_module_version = #version))
        } else {
            syn::parse_quote!(#path(#tokens, __schema_module_version = #version))
        };
        return Ok(());
    }

    Ok(())
}
