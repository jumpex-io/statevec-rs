// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use quote::quote;

use crate::field_parser::{ParsedField, ParsedTypeKind};

pub(crate) fn gen_pk_push(f: &ParsedField) -> proc_macro2::TokenStream {
    let ident = &f.ident;
    match &f.ty_kind {
        ParsedTypeKind::Bool => {
            quote! { __pk.push_u8(if acc.#ident() { 1 } else { 0 }); }
        }
        ParsedTypeKind::U8 => quote! { __pk.push_u8(acc.#ident()); },
        ParsedTypeKind::U16 => quote! { __pk.push_u16(acc.#ident()); },
        ParsedTypeKind::U32 => quote! { __pk.push_u32(acc.#ident()); },
        ParsedTypeKind::U64 => quote! { __pk.push_u64(acc.#ident()); },
        ParsedTypeKind::I32 => quote! { __pk.push_i32(acc.#ident()); },
        ParsedTypeKind::I64 => quote! { __pk.push_i64(acc.#ident()); },
        ParsedTypeKind::FixedBytes { .. } => {
            quote! { __pk.push_bytes(acc.#ident().padded_slice()); }
        }
        ParsedTypeKind::EnumU8 => quote! { __pk.push_u8(acc.#ident().to_u8()); },
        ParsedTypeKind::U128 => quote! {
            compile_error!("u128 is not supported in PK v1");
        },
        ParsedTypeKind::VarBytes => unreachable!("VarBytes rejected by classify_type"),
    }
}

pub(crate) fn gen_pk_fn_arg(f: &ParsedField) -> proc_macro2::TokenStream {
    let ident = &f.ident;
    let ty = &f.ty;

    match &f.ty_kind {
        ParsedTypeKind::FixedBytes { .. } => quote! { #ident: &#ty },
        ParsedTypeKind::Bool
        | ParsedTypeKind::U8
        | ParsedTypeKind::U16
        | ParsedTypeKind::U32
        | ParsedTypeKind::U64
        | ParsedTypeKind::I32
        | ParsedTypeKind::I64
        | ParsedTypeKind::EnumU8 => quote! { #ident: #ty },
        ParsedTypeKind::U128 => quote! {
            compile_error!("u128 is not supported in PK v1");
        },
        ParsedTypeKind::VarBytes => unreachable!("VarBytes rejected by classify_type"),
    }
}

pub(crate) fn gen_pk_arg_push(f: &ParsedField) -> proc_macro2::TokenStream {
    let ident = &f.ident;

    match &f.ty_kind {
        ParsedTypeKind::Bool => quote! { __pk.push_u8(if #ident { 1 } else { 0 }); },
        ParsedTypeKind::U8 => quote! { __pk.push_u8(#ident); },
        ParsedTypeKind::U16 => quote! { __pk.push_u16(#ident); },
        ParsedTypeKind::U32 => quote! { __pk.push_u32(#ident); },
        ParsedTypeKind::U64 => quote! { __pk.push_u64(#ident); },
        ParsedTypeKind::I32 => quote! { __pk.push_i32(#ident); },
        ParsedTypeKind::I64 => quote! { __pk.push_i64(#ident); },
        ParsedTypeKind::FixedBytes { .. } => quote! { __pk.push_bytes(#ident.padded_slice()); },
        ParsedTypeKind::EnumU8 => quote! { __pk.push_u8(#ident.to_u8()); },
        ParsedTypeKind::U128 => quote! {
            compile_error!("u128 is not supported in PK v1");
        },
        ParsedTypeKind::VarBytes => unreachable!("VarBytes rejected by classify_type"),
    }
}
