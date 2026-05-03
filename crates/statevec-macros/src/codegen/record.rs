// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use quote::{format_ident, quote};
use syn::{Data, DataStruct, DeriveInput, Fields, spanned::Spanned};

use crate::codegen::pk::{gen_pk_arg_push, gen_pk_fn_arg, gen_pk_push};
use crate::codegen::{
    ensure_struct_derives, gen_field_definition, gen_offset_const, reject_repr, screaming,
};
use crate::field_parser::{ParsedField, ParsedTypeKind, parse_field, parse_record_args};

pub(crate) fn expand_record(
    args_ts: proc_macro2::TokenStream,
    input: DeriveInput,
) -> syn::Result<proc_macro2::TokenStream> {
    reject_repr(&input.attrs)?;

    if !input.generics.params.is_empty() {
        return Err(syn::Error::new(
            input.generics.span(),
            "#[record] does not support generics in v1",
        ));
    }

    let name = input.ident.clone();
    let access_name = format_ident!("{}Access", name);
    let access_mut_name = format_ident!("{}AccessMut", name);
    let new_builder_name = format_ident!("New{}Builder", name);
    let update_builder_name = format_ident!("Update{}Builder", name);
    let def_static_name = format_ident!("{}_RECORD_DEFINITION", screaming(&name.to_string()));

    let record_args = parse_record_args(args_ts)?;
    if record_args.kind == 0 {
        return Err(syn::Error::new(
            name.span(),
            "record kind 0 is reserved; valid range is [1,255]",
        ));
    }

    {
        let rl = record_args.record_len;
        let n = rl / 64;
        if rl == 0 || rl % 64 != 0 || !n.is_power_of_two() {
            return Err(syn::Error::new(
                name.span(),
                format!(
                    "record_len must be 64*N where N is a power of 2 (64, 128, 256, …); got {}",
                    rl
                ),
            ));
        }
    }

    let Data::Struct(DataStruct {
        fields: Fields::Named(fields_named),
        ..
    }) = &input.data
    else {
        return Err(syn::Error::new(
            input.span(),
            "#[record] only supports named-field structs",
        ));
    };

    let mut parsed_fields = Vec::<ParsedField>::new();
    for f in fields_named.named.iter() {
        parsed_fields.push(parse_field(f)?);
    }

    parsed_fields.sort_by_key(|f| f.index);

    for w in parsed_fields.windows(2) {
        if w[0].index == w[1].index {
            return Err(syn::Error::new(
                w[1].ident.span(),
                format!("duplicate field index {}", w[0].index),
            ));
        }
    }

    for (i, f) in parsed_fields.iter().enumerate() {
        let expected = (i + 1) as u32;
        if f.index != expected {
            return Err(syn::Error::new(
                f.ident.span(),
                format!(
                    "field indexes must be contiguous starting at 1; expected {}",
                    expected
                ),
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
    let is_pk_idx = !record_args.pk_fields.is_empty();

    // validate pk fields
    for pk in record_args.pk_fields.iter() {
        let Some(found) = parsed_fields.iter().find(|f| f.ident == *pk) else {
            return Err(syn::Error::new(
                pk.span(),
                format!("pk field '{}' not found", pk),
            ));
        };
        if found.reserved {
            return Err(syn::Error::new(
                pk.span(),
                format!("pk field '{}' cannot be reserved", pk),
            ));
        }
        if !found.pk_supported {
            return Err(syn::Error::new(
                pk.span(),
                format!("field '{}' type is not supported in PK v1", pk),
            ));
        }
        if !found.immutable {
            return Err(syn::Error::new(
                pk.span(),
                format!("pk field '{}' must be declared #[field(immutable)]", pk),
            ));
        }
    }

    let mut clean_input = input.clone();
    if let Data::Struct(DataStruct {
        fields: Fields::Named(ref mut fields_named),
        ..
    }) = clean_input.data
    {
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
    let new_builder_setters = active_fields
        .iter()
        .filter_map(|f| gen_new_builder_setter(f));
    let update_builder_getters = active_fields.iter().map(|f| gen_builder_getter(f));
    let update_builder_setters = active_fields
        .iter()
        .filter_map(|f| gen_update_builder_setter(f));
    let offset_consts = active_fields.iter().map(|f| gen_offset_const(f));

    let field_defs = active_fields.iter().copied().map(gen_field_definition);
    let reserved_field_defs = parsed_fields
        .iter()
        .filter(|f| f.reserved)
        .map(gen_field_definition);

    let pk_pushes = record_args.pk_fields.iter().map(|pk_ident| {
        let f = parsed_fields.iter().find(|f| f.ident == *pk_ident).unwrap();
        gen_pk_push(f)
    });
    let pk_fn_args = record_args.pk_fields.iter().map(|pk_ident| {
        let f = parsed_fields.iter().find(|f| f.ident == *pk_ident).unwrap();
        gen_pk_fn_arg(f)
    });
    let pk_fn_pushes = record_args.pk_fields.iter().map(|pk_ident| {
        let f = parsed_fields.iter().find(|f| f.ident == *pk_ident).unwrap();
        gen_pk_arg_push(f)
    });

    let pk_field_strs: Vec<_> = record_args
        .pk_fields
        .iter()
        .map(|ident| {
            let s = ident.to_string();
            quote! { #s }
        })
        .collect();

    let kind = record_args.kind;
    let record_len = record_args.record_len;
    let schema_version = record_args.version;
    let (pk_encode_expr, pk_codec_impl) = if is_pk_idx {
        let expr = quote! {
            ::core::option::Option::Some(<#name as statevec_model::PkCodec>::encode_pk_from_bytes)
        };
        let impl_block = quote! {
            impl statevec_model::PkCodec for #name {
                #[inline]
                fn encode_pk_from_bytes(data: &[u8]) -> statevec_model::PkBytes {
                    let acc = #access_name::new(data);
                    let mut __pk = statevec_model::PkBuilder::new();
                    #( #pk_pushes )*
                    __pk.finish()
                }
            }
        };
        (expr, impl_block)
    } else {
        (quote! { ::core::option::Option::None }, quote! {})
    };
    let pk_fn_impl = if is_pk_idx {
        quote! {
            impl #name {
                #[inline]
                pub fn pk(#(#pk_fn_args),*) -> statevec_model::PkBytes {
                    let mut __pk = statevec_model::PkBuilder::new();
                    #( #pk_fn_pushes )*
                    __pk.finish()
                }
            }
        }
    } else {
        quote! {}
    };
    let expanded = quote! {
        #original_struct

        #pk_fn_impl

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
            is_pk_idx: #is_pk_idx,
            support_range_scan: false,
            data_size: #data_len as u32,
            version: #schema_version,
            pk_encode: #pk_encode_expr,
            fields: &[
                #( #field_defs, )*
            ],
            reserved_fields: &[
                #( #reserved_field_defs, )*
            ],
            pk_fields: &[#( #pk_field_strs, )*],
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

        #pk_codec_impl
    };

    Ok(expanded)
}

pub(crate) fn gen_ro_getter(f: &ParsedField) -> proc_macro2::TokenStream {
    gen_ro_getter_impl(f, quote! { self.0 })
}

pub(crate) fn gen_ro_getter_mut_side(f: &ParsedField) -> proc_macro2::TokenStream {
    gen_ro_getter_impl(f, quote! { self.0 })
}

pub(crate) fn gen_ro_getter_impl(
    f: &ParsedField,
    buf: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
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

pub(crate) fn gen_builder_setter(
    f: &ParsedField,
    allow_immutable: bool,
) -> Option<proc_macro2::TokenStream> {
    if f.immutable && !allow_immutable {
        return None;
    }

    let ident = &f.ident;
    let setter = if f.immutable {
        format_ident!("init_{}", ident)
    } else {
        format_ident!("set_{}", ident)
    };
    let raw_setter = if f.immutable {
        format_ident!("init_{}_raw", ident)
    } else {
        format_ident!("set_{}_raw", ident)
    };
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
    let setter = if f.immutable {
        format_ident!("init_{}", ident)
    } else {
        format_ident!("set_{}", ident)
    };
    let raw_setter = if f.immutable {
        format_ident!("init_{}_raw", ident)
    } else {
        format_ident!("set_{}_raw", ident)
    };
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
