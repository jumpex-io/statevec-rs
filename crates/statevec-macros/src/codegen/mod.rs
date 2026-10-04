// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

pub(crate) mod command_dispatch;
mod command_preflight;
pub(crate) mod enum_u8;
pub(crate) mod export_plugin;
pub(crate) mod payload;
pub(crate) mod record;
pub(crate) mod schema_module;
pub(crate) mod uk;

use quote::{ToTokens, format_ident, quote};
use syn::{Attribute, DeriveInput, Path, Token, punctuated::Punctuated, spanned::Spanned};

use crate::field_parser::ParsedField;

/// Check scalar values whose generated getters are infallible. Other fixed
/// scalars need only the enclosing layout's bounds check.
pub(crate) fn gen_checked_scalar_value(
    kind: &crate::field_parser::ParsedTypeKind,
    ty: &syn::Type,
    data: proc_macro2::TokenStream,
    offset: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    use crate::field_parser::ParsedTypeKind;
    match kind {
        ParsedTypeKind::EnumU8 => quote! {
            <#ty as statevec_model::RepeatedElement>::read_element(#data, #offset)?;
        },
        ParsedTypeKind::FixedBytes { n } => quote! {
            let __length = usize::from(statevec_model::read_u16_le(#data, #offset)?);
            if __length > #n {
                return ::core::result::Result::Err(statevec_model::AccessError { required: __length, actual: #n });
            }
        },
        _ => quote! {},
    }
}

pub(crate) fn ensure_struct_derives(input: &mut DeriveInput, required: &[&str]) -> syn::Result<()> {
    let mut collected: Vec<Path> = Vec::new();
    let mut collected_keys: Vec<String> = Vec::new();
    let mut kept_attrs: Vec<Attribute> = Vec::new();

    for attr in input.attrs.drain(..) {
        if !attr.path().is_ident("derive") {
            kept_attrs.push(attr);
            continue;
        }

        let paths = attr.parse_args_with(Punctuated::<Path, Token![,]>::parse_terminated)?;
        for path in paths {
            let key = path.to_token_stream().to_string();
            if !collected_keys.iter().any(|existing| existing == &key) {
                collected_keys.push(key);
                collected.push(path);
            }
        }
    }

    for trait_name in required {
        let path: Path = syn::parse_str(trait_name)?;
        let key = path.to_token_stream().to_string();
        if !collected_keys.iter().any(|existing| existing == &key) {
            collected_keys.push(key);
            collected.push(path);
        }
    }

    if !collected.is_empty() {
        let derive_attr: Attribute = syn::parse_quote! {
            #[derive(#(#collected),*)]
        };
        kept_attrs.insert(0, derive_attr);
    }

    input.attrs = kept_attrs;
    Ok(())
}

pub(crate) fn reject_repr(attrs: &[Attribute]) -> syn::Result<()> {
    for attr in attrs {
        if attr.path().is_ident("repr") {
            return Err(syn::Error::new(attr.span(), "#[record] declaration struct must not use #[repr(...)]"));
        }
    }
    Ok(())
}

pub(crate) fn screaming(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    for (i, &ch) in chars.iter().enumerate() {
        if ch.is_uppercase() && i > 0 {
            let prev_lower = chars[i - 1].is_lowercase();
            let next_lower = chars.get(i + 1).is_some_and(|c| c.is_lowercase());
            if prev_lower || next_lower {
                out.push('_');
            }
        }
        out.push(ch.to_ascii_uppercase());
    }
    out
}

pub(crate) fn gen_offset_const(f: &ParsedField) -> proc_macro2::TokenStream {
    let const_name = format_ident!("{}_OFFSET", f.ident.to_string().to_ascii_uppercase());
    let offset = f.offset;
    quote! {
        pub const #const_name: usize = #offset;
    }
}

pub(crate) fn gen_field_definition(f: &ParsedField) -> proc_macro2::TokenStream {
    use crate::field_parser::ParsedTypeKind;

    let name = f.ident.to_string();
    let index = f.index;
    let offset = f.offset as u32;
    let len = f.fixed_size as u32;
    let ty = &f.ty;
    let rust_type_name = quote!(::core::stringify!(#ty));
    let immutable = f.immutable;
    let semantic = semantic_token(f.semantic);
    let (ty_variant, rust_type_name, enum_type_name, decimal_scale) = match &f.ty_kind {
        ParsedTypeKind::Bool => (
            quote! { statevec_model::FieldType::Bool },
            quote! { ::core::stringify!(bool) },
            quote! { ::core::option::Option::None },
            quote! { ::core::option::Option::None },
        ),
        ParsedTypeKind::U8 => (
            quote! { statevec_model::FieldType::U8 },
            quote! { ::core::stringify!(u8) },
            quote! { ::core::option::Option::None },
            quote! { ::core::option::Option::None },
        ),
        ParsedTypeKind::U16 => (
            quote! { statevec_model::FieldType::U16 },
            quote! { ::core::stringify!(u16) },
            quote! { ::core::option::Option::None },
            quote! { ::core::option::Option::None },
        ),
        ParsedTypeKind::U32 => (
            quote! { statevec_model::FieldType::U32 },
            quote! { ::core::stringify!(u32) },
            quote! { ::core::option::Option::None },
            quote! { ::core::option::Option::None },
        ),
        ParsedTypeKind::U64 => (
            quote! { statevec_model::FieldType::U64 },
            quote! { ::core::stringify!(u64) },
            quote! { ::core::option::Option::None },
            quote! { ::core::option::Option::None },
        ),
        ParsedTypeKind::I32 => (
            quote! { statevec_model::FieldType::I32 },
            quote! { ::core::stringify!(i32) },
            quote! { ::core::option::Option::None },
            quote! { ::core::option::Option::None },
        ),
        ParsedTypeKind::I64 => (
            quote! { statevec_model::FieldType::I64 },
            quote! { ::core::stringify!(i64) },
            quote! { ::core::option::Option::None },
            quote! { ::core::option::Option::None },
        ),
        ParsedTypeKind::U128 => (
            quote! { statevec_model::FieldType::U128 },
            quote! { ::core::stringify!(u128) },
            quote! { ::core::option::Option::None },
            quote! { ::core::option::Option::None },
        ),
        ParsedTypeKind::FixedBytes { .. } => (
            quote! { statevec_model::FieldType::FixedBytes },
            rust_type_name,
            quote! { ::core::option::Option::None },
            quote! { ::core::option::Option::None },
        ),
        ParsedTypeKind::EnumU8 => (
            quote! { statevec_model::FieldType::EnumU8 },
            quote! { ::core::stringify!(#ty) },
            quote! { ::core::option::Option::Some(<#ty as statevec_model::EnumU8>::DEFINITION.name) },
            quote! { ::core::option::Option::None },
        ),
        ParsedTypeKind::Decimal { scale } => {
            let scale = *scale;
            (
                quote! { statevec_model::FieldType::Decimal },
                rust_type_name,
                quote! { ::core::option::Option::None },
                quote! { ::core::option::Option::Some(#scale) },
            )
        }
        ParsedTypeKind::VarBytes => unreachable!("VarBytes rejected by classify_type"),
    };

    quote! {
        statevec_model::FieldDefinition {
            name: #name,
            field_index: #index,
            offset: #offset,
            ty: #ty_variant,
            len: #len,
            rust_type_name: #rust_type_name,
            enum_type_name: #enum_type_name,
            decimal_scale: #decimal_scale,
            semantic: #semantic,
            immutable: #immutable,
        }
    }
}

pub(crate) fn semantic_token(semantic: Option<statevec_model::SemanticTag>) -> proc_macro2::TokenStream {
    match semantic {
        Some(statevec_model::SemanticTag::Text) => {
            quote! { ::core::option::Option::Some(statevec_model::SemanticTag::Text) }
        }
        Some(statevec_model::SemanticTag::TimestampMicros) => {
            quote! { ::core::option::Option::Some(statevec_model::SemanticTag::TimestampMicros) }
        }
        Some(statevec_model::SemanticTag::Uuid) => {
            quote! { ::core::option::Option::Some(statevec_model::SemanticTag::Uuid) }
        }
        None => quote! { ::core::option::Option::None },
    }
}
