// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use quote::{format_ident, quote};
use syn::{Data, DataStruct, DeriveInput, Fields, spanned::Spanned};

use crate::codegen::uk::{gen_uk_arg_push, gen_uk_fn_arg, gen_uk_push};
use crate::codegen::{ensure_struct_derives, gen_field_definition, gen_offset_const, reject_repr, screaming};
use crate::field_parser::{ParsedField, ParsedTypeKind, parse_field, parse_record_args};

pub(crate) fn expand_record(
    args_ts: proc_macro2::TokenStream,
    input: DeriveInput,
) -> syn::Result<proc_macro2::TokenStream> {
    reject_repr(&input.attrs)?;

    if !input.generics.params.is_empty() {
        return Err(syn::Error::new(input.generics.span(), "#[record] does not support generics in v1"));
    }

    let name = input.ident.clone();
    let access_name = format_ident!("{}Access", name);
    let access_mut_name = format_ident!("{}AccessMut", name);
    let new_builder_name = format_ident!("New{}Builder", name);
    let update_builder_name = format_ident!("Update{}Builder", name);
    let def_static_name = format_ident!("{}_RECORD_DEFINITION", screaming(&name.to_string()));

    let record_args = parse_record_args(args_ts)?;
    if record_args.kind == 0 {
        return Err(syn::Error::new(name.span(), "record kind 0 is reserved; valid range is [1,61439]"));
    }
    if record_args.kind >= statevec_model::SYSTEM_KIND_MIN {
        return Err(syn::Error::new(
            name.span(),
            format!(
                "record kind {} is reserved for StateVec system metadata; valid range is [1,61439]",
                record_args.kind
            ),
        ));
    }

    {
        let rl = record_args.record_len;
        let n = rl / 64;
        if rl == 0 || rl % 64 != 0 || !n.is_power_of_two() {
            return Err(syn::Error::new(
                name.span(),
                format!("record_len must be 64*N where N is a power of 2 (64, 128, 256, …); got {}", rl),
            ));
        }
        if rl > statevec_model::MAX_RECORD_LEN {
            return Err(syn::Error::new(
                name.span(),
                format!(
                    "record_len exceeds the runtime limit of {} bytes (including header); got {}",
                    statevec_model::MAX_RECORD_LEN,
                    rl,
                ),
            ));
        }
    }

    let Data::Struct(DataStruct { fields: Fields::Named(fields_named), .. }) = &input.data else {
        return Err(syn::Error::new(input.span(), "#[record] only supports named-field structs"));
    };

    let mut parsed_fields = Vec::<ParsedField>::new();
    for f in fields_named.named.iter() {
        parsed_fields.push(parse_field(f)?);
    }

    parsed_fields.sort_by_key(|f| f.index);

    for w in parsed_fields.windows(2) {
        if w[0].index == w[1].index {
            return Err(syn::Error::new(w[1].ident.span(), format!("duplicate field index {}", w[0].index)));
        }
    }

    for (i, f) in parsed_fields.iter().enumerate() {
        let expected = (i + 1) as u32;
        if f.index != expected {
            return Err(syn::Error::new(
                f.ident.span(),
                format!("field indexes must be contiguous starting at 1; expected {}", expected),
            ));
        }
    }

    let mut offset = 0usize;
    for f in parsed_fields.iter_mut() {
        f.offset = offset;
        offset += f.fixed_size;
    }

    let data_len = record_args.record_len - statevec_model::record::RECORD_HEADER_SIZE;
    if offset > data_len {
        return Err(syn::Error::new(
            name.span(),
            format!(
                "record_len {} is smaller than computed layout {} ({} header + {} fields)",
                record_args.record_len,
                offset + statevec_model::record::RECORD_HEADER_SIZE,
                statevec_model::record::RECORD_HEADER_SIZE,
                offset,
            ),
        ));
    }

    let active_fields: Vec<_> = parsed_fields.iter().filter(|f| !f.reserved).collect();
    let field_count = active_fields.len();
    let has_unique_keys = !record_args.unique_keys.is_empty();

    // validate uk fields
    for unique_key in &record_args.unique_keys {
        if unique_key.fields.is_empty() {
            return Err(syn::Error::new(name.span(), "uk(id = N, fields = [...]) cannot be empty"));
        }
        for uk in unique_key.fields.iter() {
            let Some(found) = parsed_fields.iter().find(|f| f.ident == *uk) else {
                return Err(syn::Error::new(uk.span(), format!("uk field '{}' not found", uk)));
            };
            if found.reserved {
                return Err(syn::Error::new(uk.span(), format!("uk field '{}' cannot be reserved", uk)));
            }
            if !found.uk_supported {
                return Err(syn::Error::new(uk.span(), format!("field '{}' type is not supported in UK v1", uk)));
            }
            if !found.immutable {
                return Err(syn::Error::new(
                    uk.span(),
                    format!("uk field '{}' must be declared #[field(immutable)]", uk),
                ));
            }
        }
    }
    // validate canonical index fields
    for index in &record_args.canonical_indexes {
        if index.fields.is_empty() {
            return Err(syn::Error::new(name.span(), "index(fields = [...]) cannot be empty"));
        }
        if index.fields.len() > statevec_model::MAX_CANONICAL_INDEX_FIELDS {
            return Err(syn::Error::new(
                name.span(),
                format!(
                    "canonical index '{}' supports at most {} fields",
                    index.name,
                    statevec_model::MAX_CANONICAL_INDEX_FIELDS
                ),
            ));
        }

        let mut encoded_len = 0usize;
        for field_ident in index.fields.iter() {
            let Some(found) = parsed_fields.iter().find(|f| f.ident == *field_ident) else {
                return Err(syn::Error::new(field_ident.span(), format!("index field '{}' not found", field_ident)));
            };
            if found.reserved {
                return Err(syn::Error::new(
                    field_ident.span(),
                    format!("index field '{}' cannot be reserved", field_ident),
                ));
            }
            if !found.uk_supported {
                return Err(syn::Error::new(
                    field_ident.span(),
                    format!("field '{}' type is not supported in canonical indexes v1", field_ident),
                ));
            }
            encoded_len += encoded_key_len(found);
        }
        if encoded_len > statevec_model::MAX_CANONICAL_INDEX_KEY_BYTES {
            return Err(syn::Error::new(
                name.span(),
                format!(
                    "canonical index '{}' encoded key is {} bytes; max is {}",
                    index.name,
                    encoded_len,
                    statevec_model::MAX_CANONICAL_INDEX_KEY_BYTES
                ),
            ));
        }
    }

    let mut clean_input = input.clone();
    if let Data::Struct(DataStruct { fields: Fields::Named(ref mut fields_named), .. }) = clean_input.data {
        for field in fields_named.named.iter_mut() {
            field.attrs.retain(|attr| !attr.path().is_ident("field"));
        }
    }
    ensure_struct_derives(&mut clean_input, &["Clone", "Debug", "PartialEq", "Eq"])?;
    let original_struct = quote! { #clean_input };

    let access_getters = active_fields.iter().map(|f| gen_ro_getter(f));
    let access_mut_getters = active_fields.iter().map(|f| gen_ro_getter_mut_side(f));
    let access_mut_setters = active_fields.iter().map(|f| gen_setter(f));
    let new_builder_getters = active_fields.iter().map(|f| gen_builder_getter(f));
    let new_builder_setters = active_fields.iter().filter_map(|f| gen_new_builder_setter(f));
    let update_builder_getters = active_fields.iter().map(|f| gen_builder_getter(f));
    let update_builder_setters = active_fields.iter().filter_map(|f| gen_update_builder_setter(f));
    let offset_consts = active_fields.iter().map(|f| gen_offset_const(f));

    let field_defs = active_fields.iter().copied().map(gen_field_definition);
    let reserved_field_defs = parsed_fields.iter().filter(|f| f.reserved).map(gen_field_definition);

    let mut uk_encode_fns = Vec::new();
    let mut uk_public_fns = Vec::new();
    let mut unique_key_defs = Vec::new();
    let mut index_encode_fns = Vec::new();
    let mut index_public_fns = Vec::new();
    let mut canonical_index_defs = Vec::new();

    for unique_key in record_args.unique_keys.iter() {
        let uk_id = unique_key.id;
        let encode_fn = format_ident!("__statevec_encode_uk_{}_from_bytes", uk_id);
        let uk_pushes = unique_key.fields.iter().map(|uk_ident| {
            let f = parsed_fields.iter().find(|f| f.ident == *uk_ident).unwrap();
            gen_uk_push(f)
        });
        let uk_field_strs: Vec<_> = unique_key
            .fields
            .iter()
            .map(|ident| {
                let s = ident.to_string();
                quote! { #s }
            })
            .collect();
        let uk_name = unique_key.name.as_deref().unwrap_or("");

        let uk_id_const = unique_key
            .name
            .as_deref()
            .map(|name| format_ident!("{}_UK_ID", screaming(name)))
            .unwrap_or_else(|| format_ident!("UK_ID"));
        uk_public_fns.push(quote! {
            pub const #uk_id_const: u8 = #uk_id;
        });

        uk_encode_fns.push(quote! {
            #[inline]
            fn #encode_fn(data: &[u8]) -> statevec_model::KeyBytes {
                let acc = #access_name::new(data);
                let mut __uk = statevec_model::KeyBuilder::new();
                #( #uk_pushes )*
                __uk.finish()
            }
        });

        let uk_fn_args_for_public = unique_key.fields.iter().map(|uk_ident| {
            let f = parsed_fields.iter().find(|f| f.ident == *uk_ident).unwrap();
            gen_uk_fn_arg(f)
        });
        let uk_fn_pushes_for_public = unique_key.fields.iter().map(|uk_ident| {
            let f = parsed_fields.iter().find(|f| f.ident == *uk_ident).unwrap();
            gen_uk_arg_push(f)
        });
        if uk_id == 0 {
            uk_public_fns.push(quote! {
                #[inline]
                pub fn uk(#(#uk_fn_args_for_public),*) -> statevec_model::KeyBytes {
                    let mut __uk = statevec_model::KeyBuilder::new();
                    #( #uk_fn_pushes_for_public )*
                    __uk.finish()
                }
            });
        }

        if let Some(uk_name) = unique_key.name.as_deref() {
            let fn_name = syn::parse_str::<syn::Ident>(&format!("uk_{uk_name}")).map_err(|_| {
                syn::Error::new(name.span(), format!("uk name '{uk_name}' cannot be used as a Rust helper name"))
            })?;
            let named_uk_fn_args = unique_key.fields.iter().map(|uk_ident| {
                let f = parsed_fields.iter().find(|f| f.ident == *uk_ident).unwrap();
                gen_uk_fn_arg(f)
            });
            let named_uk_fn_pushes = unique_key.fields.iter().map(|uk_ident| {
                let f = parsed_fields.iter().find(|f| f.ident == *uk_ident).unwrap();
                gen_uk_arg_push(f)
            });
            uk_public_fns.push(quote! {
                #[inline]
                pub fn #fn_name(#(#named_uk_fn_args),*) -> statevec_model::KeyBytes {
                    let mut __uk = statevec_model::KeyBuilder::new();
                    #( #named_uk_fn_pushes )*
                    __uk.finish()
                }
            });
        }

        unique_key_defs.push(quote! {
            statevec_model::UniqueKeyDefinition {
                id: #uk_id,
                name: #uk_name,
                encode: ::core::option::Option::Some(#name::#encode_fn),
                fields: &[#( #uk_field_strs, )*],
            }
        });
    }

    for index in record_args.canonical_indexes.iter() {
        let index_id = index.id;
        let encode_fn = format_ident!("__statevec_encode_index_{}_from_bytes", index_id);
        let index_pushes = index.fields.iter().map(|field_ident| {
            let f = parsed_fields.iter().find(|f| f.ident == *field_ident).unwrap();
            gen_uk_push(f)
        });
        let index_field_strs: Vec<_> = index
            .fields
            .iter()
            .map(|ident| {
                let s = ident.to_string();
                quote! { #s }
            })
            .collect();
        let index_name = &index.name;
        let index_id_const = format_ident!("{}_INDEX_ID", screaming(index_name));
        let index_fn_name = syn::parse_str::<syn::Ident>(&format!("index_{index_name}")).map_err(|_| {
            syn::Error::new(name.span(), format!("index name '{index_name}' cannot be used as a Rust helper name"))
        })?;
        let index_fn_args = index.fields.iter().map(|field_ident| {
            let f = parsed_fields.iter().find(|f| f.ident == *field_ident).unwrap();
            gen_uk_fn_arg(f)
        });
        let index_fn_pushes = index.fields.iter().map(|field_ident| {
            let f = parsed_fields.iter().find(|f| f.ident == *field_ident).unwrap();
            gen_uk_arg_push(f)
        });

        index_public_fns.push(quote! {
            pub const #index_id_const: u8 = #index_id;

            #[inline]
            pub fn #index_fn_name(#(#index_fn_args),*) -> statevec_model::KeyBytes {
                let mut __uk = statevec_model::KeyBuilder::new();
                #( #index_fn_pushes )*
                __uk.finish()
            }
        });

        for prefix_len in 1..index.fields.len() {
            let prefix_fields = &index.fields[..prefix_len];
            let prefix_fn_name = syn::parse_str::<syn::Ident>(&format!("index_{index_name}_prefix{prefix_len}"))
                .map_err(|_| {
                    syn::Error::new(
                        name.span(),
                        format!("index name '{index_name}' cannot be used as a Rust prefix helper name"),
                    )
                })?;
            let prefix_fn_args = prefix_fields.iter().map(|field_ident| {
                let f = parsed_fields.iter().find(|f| f.ident == *field_ident).unwrap();
                gen_uk_fn_arg(f)
            });
            let prefix_fn_pushes = prefix_fields.iter().map(|field_ident| {
                let f = parsed_fields.iter().find(|f| f.ident == *field_ident).unwrap();
                gen_uk_arg_push(f)
            });
            index_public_fns.push(quote! {
                #[inline]
                pub fn #prefix_fn_name(#(#prefix_fn_args),*) -> statevec_model::KeyBytes {
                    let mut __uk = statevec_model::KeyBuilder::new();
                    #( #prefix_fn_pushes )*
                    __uk.finish()
                }
            });
        }

        index_encode_fns.push(quote! {
            #[inline]
            fn #encode_fn(data: &[u8]) -> statevec_model::KeyBytes {
                let acc = #access_name::new(data);
                let mut __uk = statevec_model::KeyBuilder::new();
                #( #index_pushes )*
                __uk.finish()
            }
        });

        canonical_index_defs.push(quote! {
            statevec_model::CanonicalIndexDefinition {
                id: #index_id,
                name: #index_name,
                encode: ::core::option::Option::Some(#name::#encode_fn),
                fields: &[#( #index_field_strs, )*],
            }
        });
    }

    let kind = record_args.kind;
    let record_len = record_args.record_len;
    let schema_version = record_args.version;
    let uk_codec_impl = if has_unique_keys {
        quote! {
            impl statevec_model::UkCodec for #name {
                #[inline]
                fn encode_uk_from_bytes(data: &[u8]) -> statevec_model::KeyBytes {
                    #name::__statevec_encode_uk_0_from_bytes(data)
                }
            }
        }
    } else {
        quote! {}
    };
    let key_fn_impl = if has_unique_keys || !record_args.canonical_indexes.is_empty() {
        quote! {
            impl #name {
                #( #uk_encode_fns )*
                #( #uk_public_fns )*
                #( #index_encode_fns )*
                #( #index_public_fns )*
            }
        }
    } else {
        quote! {}
    };
    let checked_values = active_fields.iter().map(|field| {
        let offset = field.offset;
        super::gen_checked_scalar_value(&field.ty_kind, &field.ty, quote! { buf }, quote! { #offset })
    });
    let expanded = quote! {
        #original_struct

        #key_fn_impl

        pub struct #access_name<'a>(pub &'a [u8]);

        impl<'a> #access_name<'a> {
            pub const LEN: usize = #data_len;

            #[inline(always)]
            pub fn new(buf: &'a [u8]) -> Self {
                assert!(buf.len() >= Self::LEN);
                Self(buf)
            }

            #[inline]
            pub fn try_new(buf: &'a [u8]) -> ::core::result::Result<Self, statevec_model::AccessError> {
                if buf.len() < Self::LEN {
                    return ::core::result::Result::Err(statevec_model::AccessError {
                        required: Self::LEN,
                        actual: buf.len(),
                    });
                }
                #( #checked_values )*
                ::core::result::Result::Ok(Self(buf))
            }

            #( #offset_consts )*
            #( #access_getters )*
        }

        struct #access_mut_name<'a>(pub &'a mut [u8]);

        impl<'a> #access_mut_name<'a> {
            const LEN: usize = #data_len;

            #[inline(always)]
            fn new(buf: &'a mut [u8]) -> Self {
                assert!(buf.len() >= Self::LEN);
                Self(buf)
            }

            #[inline]
            fn try_new(buf: &'a mut [u8]) -> ::core::result::Result<Self, statevec_model::AccessError> {
                if buf.len() < Self::LEN {
                    return ::core::result::Result::Err(statevec_model::AccessError {
                        required: Self::LEN,
                        actual: buf.len(),
                    });
                }
                ::core::result::Result::Ok(Self(buf))
            }

            #( #access_mut_getters )*
            #( #access_mut_setters )*
        }

        pub struct #new_builder_name<'a> {
            acc: #access_mut_name<'a>,
        }

        impl<'a> #new_builder_name<'a> {
            #[inline(always)]
            fn new(acc: #access_mut_name<'a>) -> Self {
                Self { acc }
            }

            #( #new_builder_getters )*
            #( #new_builder_setters )*
        }

        pub struct #update_builder_name<'a> {
            acc: #access_mut_name<'a>,
        }

        impl<'a> #update_builder_name<'a> {
            #[inline(always)]
            fn new(acc: #access_mut_name<'a>) -> Self {
                Self { acc }
            }

            #( #update_builder_getters )*
            #( #update_builder_setters )*
        }

        pub static #def_static_name: statevec_model::RecordDefinition = statevec_model::RecordDefinition {
            kind: #kind,
            name: ::core::stringify!(#name),
            data_size: #data_len as u32,
            version: #schema_version,
            unique_keys: &[
                #( #unique_key_defs, )*
            ],
            canonical_indexes: &[
                #( #canonical_index_defs, )*
            ],
            fields: &[
                #( #field_defs, )*
            ],
            reserved_fields: &[
                #( #reserved_field_defs, )*
            ],
        };

        impl statevec_model::RecordSchema for #name {
            const KIND: statevec_model::RecordKind = #kind;
            const RECORD_LEN: usize = #record_len;
            const FIELD_COUNT: usize = #field_count;

            #[inline(always)]
            fn definition() -> &'static statevec_model::RecordDefinition {
                &#def_static_name
            }
        }

        impl statevec_model::GeneratedRecordAccess for #name {
            const DATA_LEN: usize = #data_len;
            type Access<'a> = #access_name<'a>;
            type NewBuilder<'a> = #new_builder_name<'a>;
            type UpdateBuilder<'a> = #update_builder_name<'a>;

            #[inline(always)]
            fn wrap<'a>(buf: &'a [u8]) -> Self::Access<'a> {
                #access_name::new(buf)
            }

            #[inline(always)]
            fn wrap_new<'a>(buf: &'a mut [u8]) -> Self::NewBuilder<'a> {
                #new_builder_name::new(#access_mut_name::new(buf))
            }

            #[inline(always)]
            fn wrap_update<'a>(buf: &'a mut [u8]) -> Self::UpdateBuilder<'a> {
                #update_builder_name::new(#access_mut_name::new(buf))
            }
        }

        #uk_codec_impl
    };

    Ok(expanded)
}

pub(crate) fn gen_ro_getter(f: &ParsedField) -> proc_macro2::TokenStream {
    gen_ro_getter_impl(f, quote! { self.0 })
}

fn encoded_key_len(f: &ParsedField) -> usize {
    match &f.ty_kind {
        ParsedTypeKind::Bool | ParsedTypeKind::U8 | ParsedTypeKind::EnumU8 => 1,
        ParsedTypeKind::U16 => 2,
        ParsedTypeKind::U32 | ParsedTypeKind::I32 => 4,
        ParsedTypeKind::U64 | ParsedTypeKind::I64 => 8,
        ParsedTypeKind::FixedBytes { n } => *n,
        ParsedTypeKind::U128 | ParsedTypeKind::VarBytes | ParsedTypeKind::Decimal { .. } => 0,
    }
}

pub(crate) fn gen_ro_getter_mut_side(f: &ParsedField) -> proc_macro2::TokenStream {
    gen_ro_getter_impl(f, quote! { self.0 })
}

pub(crate) fn gen_ro_getter_impl(f: &ParsedField, buf: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    let ident = &f.ident;
    let offset = f.offset;
    let ty = &f.ty;

    match &f.ty_kind {
        ParsedTypeKind::Bool => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> bool {
                statevec_model::read_bool(#buf, #offset)
                    .expect("record accessor: buffer invariant violated")
            }
        },
        ParsedTypeKind::U8 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> u8 {
                statevec_model::read_u8(#buf, #offset)
                    .expect("record accessor: buffer invariant violated")
            }
        },
        ParsedTypeKind::U16 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> u16 {
                statevec_model::read_u16_le(#buf, #offset)
                    .expect("record accessor: buffer invariant violated")
            }
        },
        ParsedTypeKind::U32 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> u32 {
                statevec_model::read_u32_le(#buf, #offset)
                    .expect("record accessor: buffer invariant violated")
            }
        },
        ParsedTypeKind::U64 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> u64 {
                statevec_model::read_u64_le(#buf, #offset)
                    .expect("record accessor: buffer invariant violated")
            }
        },
        ParsedTypeKind::I32 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> i32 {
                statevec_model::read_i32_le(#buf, #offset)
                    .expect("record accessor: buffer invariant violated")
            }
        },
        ParsedTypeKind::I64 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> i64 {
                statevec_model::read_i64_le(#buf, #offset)
                    .expect("record accessor: buffer invariant violated")
            }
        },
        ParsedTypeKind::U128 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> u128 {
                statevec_model::read_u128_le(#buf, #offset)
                    .expect("record accessor: buffer invariant violated")
            }
        },
        ParsedTypeKind::FixedBytes { n } => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> statevec_model::FixedBytes<#n> {
                statevec_model::read_fixed_bytes::<#n>(#buf, #offset)
                    .expect("record accessor: buffer invariant violated")
            }
        },
        ParsedTypeKind::Decimal { scale } => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> statevec_model::Decimal<#scale> {
                statevec_model::read_decimal_le::<#scale>(#buf, #offset)
                    .expect("record accessor: buffer invariant violated")
            }
        },
        ParsedTypeKind::EnumU8 => {
            let raw_ident = format_ident!("{}_raw", ident);
            quote! {
                #[inline(always)]
                pub fn #ident(&self) -> #ty {
                    let raw = statevec_model::read_u8(#buf, #offset)
                        .expect("record accessor: buffer invariant violated");
                    <#ty as statevec_model::EnumU8>::try_from_u8(raw)
                        .expect("record accessor: enum discriminant invariant violated")
                }

                #[inline(always)]
                pub fn #raw_ident(&self) -> u8 {
                    statevec_model::read_u8(#buf, #offset)
                        .expect("record accessor: buffer invariant violated")
                }
            }
        }
        ParsedTypeKind::VarBytes => unreachable!("VarBytes rejected by classify_type"),
    }
}

pub(crate) fn gen_builder_getter(f: &ParsedField) -> proc_macro2::TokenStream {
    let ident = &f.ident;
    let ty = &f.ty;

    match &f.ty_kind {
        ParsedTypeKind::Bool => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> bool {
                self.acc.#ident()
            }
        },
        ParsedTypeKind::U8 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> u8 {
                self.acc.#ident()
            }
        },
        ParsedTypeKind::U16 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> u16 {
                self.acc.#ident()
            }
        },
        ParsedTypeKind::U32 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> u32 {
                self.acc.#ident()
            }
        },
        ParsedTypeKind::U64 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> u64 {
                self.acc.#ident()
            }
        },
        ParsedTypeKind::I32 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> i32 {
                self.acc.#ident()
            }
        },
        ParsedTypeKind::I64 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> i64 {
                self.acc.#ident()
            }
        },
        ParsedTypeKind::U128 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> u128 {
                self.acc.#ident()
            }
        },
        ParsedTypeKind::FixedBytes { n } => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> statevec_model::FixedBytes<#n> {
                self.acc.#ident()
            }
        },
        ParsedTypeKind::Decimal { scale } => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> statevec_model::Decimal<#scale> {
                self.acc.#ident()
            }
        },
        ParsedTypeKind::EnumU8 => {
            let raw_ident = format_ident!("{}_raw", ident);
            quote! {
                #[inline(always)]
                pub fn #ident(&self) -> #ty {
                    self.acc.#ident()
                }

                #[inline(always)]
                pub fn #raw_ident(&self) -> u8 {
                    self.acc.#raw_ident()
                }
            }
        }
        ParsedTypeKind::VarBytes => unreachable!("VarBytes rejected by classify_type"),
    }
}

pub(crate) fn gen_builder_setter(f: &ParsedField, allow_immutable: bool) -> Option<proc_macro2::TokenStream> {
    if f.immutable && !allow_immutable {
        return None;
    }

    let ident = &f.ident;
    let setter = if f.immutable { format_ident!("init_{}", ident) } else { format_ident!("set_{}", ident) };
    let raw_setter = if f.immutable { format_ident!("init_{}_raw", ident) } else { format_ident!("set_{}_raw", ident) };
    let ty = &f.ty;

    Some(match &f.ty_kind {
        ParsedTypeKind::Bool => quote! {
            #[inline(always)]
            pub fn #setter(&mut self, v: bool) -> &mut Self {
                self.acc.#setter(v);
                self
            }
        },
        ParsedTypeKind::U8 => quote! {
            #[inline(always)]
            pub fn #setter(&mut self, v: u8) -> &mut Self {
                self.acc.#setter(v);
                self
            }
        },
        ParsedTypeKind::U16 => quote! {
            #[inline(always)]
            pub fn #setter(&mut self, v: u16) -> &mut Self {
                self.acc.#setter(v);
                self
            }
        },
        ParsedTypeKind::U32 => quote! {
            #[inline(always)]
            pub fn #setter(&mut self, v: u32) -> &mut Self {
                self.acc.#setter(v);
                self
            }
        },
        ParsedTypeKind::U64 => quote! {
            #[inline(always)]
            pub fn #setter(&mut self, v: u64) -> &mut Self {
                self.acc.#setter(v);
                self
            }
        },
        ParsedTypeKind::I32 => quote! {
            #[inline(always)]
            pub fn #setter(&mut self, v: i32) -> &mut Self {
                self.acc.#setter(v);
                self
            }
        },
        ParsedTypeKind::I64 => quote! {
            #[inline(always)]
            pub fn #setter(&mut self, v: i64) -> &mut Self {
                self.acc.#setter(v);
                self
            }
        },
        ParsedTypeKind::U128 => quote! {
            #[inline(always)]
            pub fn #setter(&mut self, v: u128) -> &mut Self {
                self.acc.#setter(v);
                self
            }
        },
        ParsedTypeKind::FixedBytes { n } => quote! {
            #[inline(always)]
            pub fn #setter(&mut self, v: &statevec_model::FixedBytes<#n>) -> &mut Self {
                self.acc.#setter(v);
                self
            }
        },
        ParsedTypeKind::Decimal { scale } => quote! {
            #[inline(always)]
            pub fn #setter(&mut self, v: statevec_model::Decimal<#scale>) -> &mut Self {
                self.acc.#setter(v);
                self
            }
        },
        ParsedTypeKind::EnumU8 => quote! {
            #[inline(always)]
            pub fn #setter(&mut self, v: #ty) -> &mut Self {
                self.acc.#setter(v);
                self
            }

            #[inline(always)]
            pub fn #raw_setter(&mut self, v: u8) -> &mut Self {
                self.acc.#raw_setter(v);
                self
            }
        },
        ParsedTypeKind::VarBytes => unreachable!("VarBytes rejected by classify_type"),
    })
}

pub(crate) fn gen_new_builder_setter(f: &ParsedField) -> Option<proc_macro2::TokenStream> {
    gen_builder_setter(f, true)
}

pub(crate) fn gen_update_builder_setter(f: &ParsedField) -> Option<proc_macro2::TokenStream> {
    gen_builder_setter(f, false)
}

pub(crate) fn gen_setter(f: &ParsedField) -> proc_macro2::TokenStream {
    let ident = &f.ident;
    let setter = if f.immutable { format_ident!("init_{}", ident) } else { format_ident!("set_{}", ident) };
    let raw_setter = if f.immutable { format_ident!("init_{}_raw", ident) } else { format_ident!("set_{}_raw", ident) };
    let offset = f.offset;
    let ty = &f.ty;

    match &f.ty_kind {
        ParsedTypeKind::Bool => quote! {
            #[inline(always)]
            pub fn #setter(&mut self, v: bool) {
                statevec_model::write_bool(self.0, #offset, v)
            }
        },
        ParsedTypeKind::U8 => quote! {
            #[inline(always)]
            pub fn #setter(&mut self, v: u8) {
                statevec_model::write_u8(self.0, #offset, v)
            }
        },
        ParsedTypeKind::U16 => quote! {
            #[inline(always)]
            pub fn #setter(&mut self, v: u16) {
                statevec_model::write_u16_le(self.0, #offset, v)
            }
        },
        ParsedTypeKind::U32 => quote! {
            #[inline(always)]
            pub fn #setter(&mut self, v: u32) {
                statevec_model::write_u32_le(self.0, #offset, v)
            }
        },
        ParsedTypeKind::U64 => quote! {
            #[inline(always)]
            pub fn #setter(&mut self, v: u64) {
                statevec_model::write_u64_le(self.0, #offset, v)
            }
        },
        ParsedTypeKind::I32 => quote! {
            #[inline(always)]
            pub fn #setter(&mut self, v: i32) {
                statevec_model::write_i32_le(self.0, #offset, v)
            }
        },
        ParsedTypeKind::I64 => quote! {
            #[inline(always)]
            pub fn #setter(&mut self, v: i64) {
                statevec_model::write_i64_le(self.0, #offset, v)
            }
        },
        ParsedTypeKind::U128 => quote! {
            #[inline(always)]
            pub fn #setter(&mut self, v: u128) {
                statevec_model::write_u128_le(self.0, #offset, v)
            }
        },
        ParsedTypeKind::FixedBytes { n } => quote! {
            #[inline(always)]
            pub fn #setter(&mut self, v: &statevec_model::FixedBytes<#n>) {
                statevec_model::write_fixed_bytes::<#n>(self.0, #offset, v)
            }
        },
        ParsedTypeKind::Decimal { scale } => quote! {
            #[inline(always)]
            pub fn #setter(&mut self, v: statevec_model::Decimal<#scale>) {
                statevec_model::write_decimal_le::<#scale>(self.0, #offset, v)
            }
        },
        ParsedTypeKind::EnumU8 => {
            quote! {
                #[inline(always)]
                pub fn #setter(&mut self, v: #ty) {
                    statevec_model::write_u8(self.0, #offset, <#ty as statevec_model::EnumU8>::to_u8(v))
                }

                #[inline(always)]
                pub fn #raw_setter(&mut self, v: u8) {
                    statevec_model::write_u8(self.0, #offset, v)
                }
            }
        }
        ParsedTypeKind::VarBytes => unreachable!("VarBytes rejected by classify_type"),
    }
}
