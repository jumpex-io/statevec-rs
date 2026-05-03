// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use quote::{format_ident, quote};
use syn::{Data, DataStruct, DeriveInput, Fields, spanned::Spanned};

use crate::codegen::{ensure_struct_derives, reject_repr, screaming};
use crate::field_parser::{
    ParsedTypeKind, PayloadField, PayloadKind, parse_payload_args, parse_payload_field,
};

pub(crate) fn expand_payload(
    args_ts: proc_macro2::TokenStream,
    input: DeriveInput,
    kind: PayloadKind,
) -> syn::Result<proc_macro2::TokenStream> {
    reject_repr(&input.attrs)?;

    if !input.generics.params.is_empty() {
        return Err(syn::Error::new(
            input.generics.span(),
            "payload macros do not support generics",
        ));
    }

    let payload_args = parse_payload_args(args_ts)?;
    let payload_kind_val = payload_args.kind;
    let payload_version = payload_args.version;
    let name = input.ident.clone();

    let Data::Struct(DataStruct {
        fields: Fields::Named(fields_named),
        ..
    }) = &input.data
    else {
        return Err(syn::Error::new(
            input.span(),
            "payload macros only support named-field structs",
        ));
    };

    let mut parsed_fields = Vec::<PayloadField>::new();
    for f in fields_named.named.iter() {
        parsed_fields.push(parse_payload_field(f)?);
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

    let has_var = parsed_fields.iter().any(|f| f.is_var());
    let field_count = parsed_fields.len();

    // Clean input struct (strip #[field(...)] attrs)
    let mut clean_input = input.clone();
    if let Data::Struct(DataStruct {
        fields: Fields::Named(ref mut fnamed),
        ..
    }) = clean_input.data
    {
        for field in fnamed.named.iter_mut() {
            field.attrs.retain(|attr| !attr.path().is_ident("field"));
            if let Some(parsed) = parsed_fields
                .iter()
                .find(|parsed| field.ident.as_ref() == Some(&parsed.ident))
                && matches!(parsed.ty_kind, ParsedTypeKind::VarBytes)
            {
                field.ty = syn::parse_quote!(::std::vec::Vec<u8>);
            }
        }
    }
    ensure_struct_derives(&mut clean_input, &["Clone", "Debug", "PartialEq", "Eq"])?;
    let original_struct = quote! { #clean_input };

    let access_name = format_ident!("{}Access", name);
    let builder_name = format_ident!("{}Builder", name);
    let suffix = match kind {
        PayloadKind::Command => "COMMAND",
        PayloadKind::Event => "EVENT",
    };
    let def_static_name = format_ident!("{}_{}_DEFINITION", screaming(&name.to_string()), suffix);

    // ---- Static definition ----
    let payload_field_defs: Vec<_> = parsed_fields
        .iter()
        .map(|f| {
            let fname = f.ident.to_string();
            let fidx = f.index;
            let ty_variant = match &f.ty_kind {
                ParsedTypeKind::Bool => quote! { statevec_model::FieldType::Bool },
                ParsedTypeKind::U8 => quote! { statevec_model::FieldType::U8 },
                ParsedTypeKind::U16 => quote! { statevec_model::FieldType::U16 },
                ParsedTypeKind::U32 => quote! { statevec_model::FieldType::U32 },
                ParsedTypeKind::U64 => quote! { statevec_model::FieldType::U64 },
                ParsedTypeKind::I32 => quote! { statevec_model::FieldType::I32 },
                ParsedTypeKind::I64 => quote! { statevec_model::FieldType::I64 },
                ParsedTypeKind::U128 => quote! { statevec_model::FieldType::U128 },
                ParsedTypeKind::FixedBytes { .. } => {
                    quote! { statevec_model::FieldType::FixedBytes }
                }
                ParsedTypeKind::VarBytes => quote! { statevec_model::FieldType::VarBytes },
                ParsedTypeKind::EnumU8 => quote! { statevec_model::FieldType::EnumU8 },
            };
            let ty = &f.ty;
            let rust_type_name = quote! { ::core::stringify!(#ty) };
            let enum_type_name = match &f.ty_kind {
                ParsedTypeKind::EnumU8 => {
                    quote! { ::core::option::Option::Some(::core::stringify!(#ty)) }
                }
                _ => quote! { ::core::option::Option::None },
            };
            let fixed_size_expr = match f.fixed_size() {
                Some(n) => {
                    let n = n as u32;
                    quote! { ::core::option::Option::Some(#n) }
                }
                None => quote! { ::core::option::Option::None },
            };
            quote! {
                statevec_model::PayloadFieldDefinition {
                    name: #fname,
                    field_index: #fidx,
                    ty: #ty_variant,
                    rust_type_name: #rust_type_name,
                    enum_type_name: #enum_type_name,
                    fixed_size: #fixed_size_expr,
                }
            }
        })
        .collect();

    let pdef_type = match kind {
        PayloadKind::Command => quote! { statevec_model::CommandDefinition },
        PayloadKind::Event => quote! { statevec_model::EventDefinition },
    };

    let static_def = quote! {
        pub static #def_static_name: #pdef_type = #pdef_type {
            kind: #payload_kind_val,
            name: ::core::stringify!(#name),
            version: #payload_version,
            fields: &[
                #( #payload_field_defs, )*
            ],
        };
    };

    // ---- Access struct ----
    let access_struct = if !has_var {
        let mut offset = 0usize;
        let mut field_offsets = Vec::new();
        for f in &parsed_fields {
            field_offsets.push(offset);
            offset += f.fixed_size().unwrap();
        }
        let getters: Vec<_> = parsed_fields
            .iter()
            .zip(field_offsets.iter())
            .map(|(f, &off)| gen_payload_getter_fixed(f, off))
            .collect();

        quote! {
            pub struct #access_name<'a>(&'a [u8]);

            impl<'a> #access_name<'a> {
                #[inline(always)]
                pub fn new(data: &'a [u8]) -> Self {
                    Self(data)
                }
                #( #getters )*
            }
        }
    } else {
        let fc = field_count;
        let offset_stmts = gen_offset_stmts(&parsed_fields);
        let getters: Vec<_> = parsed_fields
            .iter()
            .enumerate()
            .map(|(i, f)| gen_payload_getter_var(f, i))
            .collect();

        quote! {
            pub struct #access_name<'a> {
                data: &'a [u8],
                offsets: [u32; #fc],
            }

            impl<'a> #access_name<'a> {
                pub fn new(data: &'a [u8]) -> Self {
                    let mut offsets = [0u32; #fc];
                    #( #offset_stmts )*
                    Self { data, offsets }
                }
                #( #getters )*
            }
        }
    };

    // ---- Builder struct ----
    let builder_field_decls: Vec<_> = parsed_fields
        .iter()
        .map(|f| {
            let ident = &f.ident;
            let bt = payload_builder_field_type(f);
            quote! { #ident: ::core::option::Option<#bt> }
        })
        .collect();

    let builder_field_inits: Vec<_> = parsed_fields
        .iter()
        .map(|f| {
            let ident = &f.ident;
            quote! { #ident: ::core::option::Option::None }
        })
        .collect();

    let builder_setters: Vec<_> = parsed_fields
        .iter()
        .map(|f| {
            let ident = &f.ident;
            let setter = format_ident!("set_{}", ident);
            let bt = payload_builder_field_type(f);
            quote! {
                pub fn #setter(mut self, v: #bt) -> Self {
                    self.#ident = ::core::option::Option::Some(v);
                    self
                }
            }
        })
        .collect();

    let build_writes: Vec<_> = parsed_fields
        .iter()
        .map(|f| {
            let ident = &f.ident;
            let msg = format!("field {} not set", f.ident);
            gen_payload_build_write(f, quote! { self.#ident.expect(#msg) })
        })
        .collect();
    let build_capacity_terms: Vec<_> = parsed_fields
        .iter()
        .map(gen_payload_build_capacity_term)
        .collect();

    let builder_struct = quote! {
        pub struct #builder_name {
            #( #builder_field_decls, )*
        }

        impl #builder_name {
            pub fn new() -> Self {
                Self {
                    #( #builder_field_inits, )*
                }
            }

            #( #builder_setters )*

            pub fn build(self) -> ::std::vec::Vec<u8> {
                let __cap: usize = 0 #( + #build_capacity_terms )*;
                let mut __buf: ::std::vec::Vec<u8> = ::std::vec::Vec::with_capacity(__cap);
                #( #build_writes )*
                __buf
            }
        }
    };

    // ---- Trait impls ----
    let (schema_trait, access_trait, def_ret_type) = match kind {
        PayloadKind::Command => (
            quote! { statevec_model::CommandSchema },
            quote! { statevec_model::GeneratedCommandAccess },
            quote! { statevec_model::CommandDefinition },
        ),
        PayloadKind::Event => (
            quote! { statevec_model::EventSchema },
            quote! { statevec_model::GeneratedEventAccess },
            quote! { statevec_model::EventDefinition },
        ),
    };

    let schema_impl = quote! {
        impl #schema_trait for #name {
            const KIND: u8 = #payload_kind_val;
            fn definition() -> &'static #def_ret_type {
                &#def_static_name
            }
        }
    };

    let generated_access_impl = quote! {
        impl #access_trait for #name {
            type Access<'a> = #access_name<'a>;
            type Builder = #builder_name;

            fn wrap(data: &[u8]) -> Self::Access<'_> {
                #access_name::new(data)
            }

            fn builder() -> Self::Builder {
                #builder_name::new()
            }
        }
    };

    Ok(quote! {
        #original_struct
        #static_def
        #access_struct
        #builder_struct
        #schema_impl
        #generated_access_impl
    })
}

pub(crate) fn payload_builder_field_type(f: &PayloadField) -> proc_macro2::TokenStream {
    let ty = &f.ty;
    match &f.ty_kind {
        ParsedTypeKind::Bool => quote! { bool },
        ParsedTypeKind::U8 => quote! { u8 },
        ParsedTypeKind::U16 => quote! { u16 },
        ParsedTypeKind::U32 => quote! { u32 },
        ParsedTypeKind::U64 => quote! { u64 },
        ParsedTypeKind::I32 => quote! { i32 },
        ParsedTypeKind::I64 => quote! { i64 },
        ParsedTypeKind::U128 => quote! { u128 },
        ParsedTypeKind::FixedBytes { n } => quote! { statevec_model::FixedBytes<#n> },
        ParsedTypeKind::VarBytes => quote! { ::std::vec::Vec<u8> },
        ParsedTypeKind::EnumU8 => quote! { #ty },
    }
}

pub(crate) fn gen_payload_build_capacity_term(f: &PayloadField) -> proc_macro2::TokenStream {
    let ident = &f.ident;
    let msg = format!("field {} not set", f.ident);
    match &f.ty_kind {
        ParsedTypeKind::Bool | ParsedTypeKind::U8 | ParsedTypeKind::EnumU8 => quote! { 1usize },
        ParsedTypeKind::U16 => quote! { 2usize },
        ParsedTypeKind::U32 | ParsedTypeKind::I32 => quote! { 4usize },
        ParsedTypeKind::U64 | ParsedTypeKind::I64 => quote! { 8usize },
        ParsedTypeKind::U128 => quote! { 16usize },
        ParsedTypeKind::FixedBytes { n } => {
            let n = *n;
            quote! { 2usize + #n }
        }
        ParsedTypeKind::VarBytes => quote! { 2usize + self.#ident.as_ref().expect(#msg).len() },
    }
}

pub(crate) fn gen_payload_build_write(
    f: &PayloadField,
    val: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    let ty = &f.ty;
    match &f.ty_kind {
        ParsedTypeKind::Bool => quote! { __buf.push(if #val { 1u8 } else { 0u8 }); },
        ParsedTypeKind::U8 => quote! { __buf.push(#val); },
        ParsedTypeKind::U16 => quote! { __buf.extend_from_slice(&(#val).to_le_bytes()); },
        ParsedTypeKind::U32 => quote! { __buf.extend_from_slice(&(#val).to_le_bytes()); },
        ParsedTypeKind::U64 => quote! { __buf.extend_from_slice(&(#val).to_le_bytes()); },
        ParsedTypeKind::I32 => quote! { __buf.extend_from_slice(&(#val).to_le_bytes()); },
        ParsedTypeKind::I64 => quote! { __buf.extend_from_slice(&(#val).to_le_bytes()); },
        ParsedTypeKind::U128 => quote! { __buf.extend_from_slice(&(#val).to_le_bytes()); },
        ParsedTypeKind::FixedBytes { .. } => quote! {
            __buf.extend_from_slice(&((#val).as_slice().len() as u16).to_le_bytes());
            __buf.extend_from_slice((#val).padded_slice());
        },
        ParsedTypeKind::VarBytes => quote! {
            let __vb = #val;
            let __vb_len = ::std::primitive::u16::try_from(__vb.len())
                .expect("VarBytes content exceeds u16::MAX");
            __buf.extend_from_slice(&__vb_len.to_le_bytes());
            __buf.extend_from_slice(&__vb);
        },
        ParsedTypeKind::EnumU8 => {
            quote! { __buf.push(<#ty as statevec_model::EnumU8>::to_u8(#val)); }
        }
    }
}

pub(crate) fn gen_payload_getter_fixed(
    f: &PayloadField,
    offset: usize,
) -> proc_macro2::TokenStream {
    let ident = &f.ident;
    let ty = &f.ty;
    match &f.ty_kind {
        ParsedTypeKind::Bool => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> bool {
                statevec_model::read_bool(self.0, #offset).expect("payload access")
            }
        },
        ParsedTypeKind::U8 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> u8 {
                statevec_model::read_u8(self.0, #offset).expect("payload access")
            }
        },
        ParsedTypeKind::U16 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> u16 {
                statevec_model::read_u16_le(self.0, #offset).expect("payload access")
            }
        },
        ParsedTypeKind::U32 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> u32 {
                statevec_model::read_u32_le(self.0, #offset).expect("payload access")
            }
        },
        ParsedTypeKind::U64 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> u64 {
                statevec_model::read_u64_le(self.0, #offset).expect("payload access")
            }
        },
        ParsedTypeKind::I32 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> i32 {
                statevec_model::read_i32_le(self.0, #offset).expect("payload access")
            }
        },
        ParsedTypeKind::I64 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> i64 {
                statevec_model::read_i64_le(self.0, #offset).expect("payload access")
            }
        },
        ParsedTypeKind::U128 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> u128 {
                statevec_model::read_u128_le(self.0, #offset).expect("payload access")
            }
        },
        ParsedTypeKind::FixedBytes { n } => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> statevec_model::FixedBytes<#n> {
                statevec_model::read_fixed_bytes::<#n>(self.0, #offset).expect("payload access")
            }
        },
        ParsedTypeKind::VarBytes => unreachable!("VarBytes not in fixed-only path"),
        ParsedTypeKind::EnumU8 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> #ty {
                let raw = statevec_model::read_u8(self.0, #offset).expect("payload access");
                <#ty as statevec_model::EnumU8>::try_from_u8(raw)
                    .unwrap_or_else(|e| panic!("{}", e))
            }
        },
    }
}

pub(crate) fn gen_payload_getter_var(f: &PayloadField, idx: usize) -> proc_macro2::TokenStream {
    let ident = &f.ident;
    let ty = &f.ty;
    match &f.ty_kind {
        ParsedTypeKind::Bool => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> bool {
                statevec_model::read_bool(self.data, self.offsets[#idx] as usize).expect("payload access")
            }
        },
        ParsedTypeKind::U8 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> u8 {
                statevec_model::read_u8(self.data, self.offsets[#idx] as usize).expect("payload access")
            }
        },
        ParsedTypeKind::U16 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> u16 {
                statevec_model::read_u16_le(self.data, self.offsets[#idx] as usize).expect("payload access")
            }
        },
        ParsedTypeKind::U32 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> u32 {
                statevec_model::read_u32_le(self.data, self.offsets[#idx] as usize).expect("payload access")
            }
        },
        ParsedTypeKind::U64 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> u64 {
                statevec_model::read_u64_le(self.data, self.offsets[#idx] as usize).expect("payload access")
            }
        },
        ParsedTypeKind::I32 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> i32 {
                statevec_model::read_i32_le(self.data, self.offsets[#idx] as usize).expect("payload access")
            }
        },
        ParsedTypeKind::I64 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> i64 {
                statevec_model::read_i64_le(self.data, self.offsets[#idx] as usize).expect("payload access")
            }
        },
        ParsedTypeKind::U128 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> u128 {
                statevec_model::read_u128_le(self.data, self.offsets[#idx] as usize).expect("payload access")
            }
        },
        ParsedTypeKind::FixedBytes { n } => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> statevec_model::FixedBytes<#n> {
                statevec_model::read_fixed_bytes::<#n>(self.data, self.offsets[#idx] as usize).expect("payload access")
            }
        },
        ParsedTypeKind::VarBytes => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> &[u8] {
                let (bytes, _) =
                    statevec_model::read_var_bytes(self.data, self.offsets[#idx] as usize)
                        .expect("payload access: var_bytes");
                bytes
            }
        },
        ParsedTypeKind::EnumU8 => quote! {
            #[inline(always)]
            pub fn #ident(&self) -> #ty {
                let raw = statevec_model::read_u8(self.data, self.offsets[#idx] as usize).expect("payload access");
                <#ty as statevec_model::EnumU8>::try_from_u8(raw)
                    .unwrap_or_else(|e| panic!("{}", e))
            }
        },
    }
}

/// Generate flat offset-computation statements for VarBytes Access::new() body.
pub(crate) fn gen_offset_stmts(fields: &[PayloadField]) -> Vec<proc_macro2::TokenStream> {
    let mut stmts = Vec::new();
    let mut compile_offset = 0usize;
    let mut cursor_created = false;

    for (i, f) in fields.iter().enumerate() {
        if !cursor_created {
            if f.is_var() {
                let co = compile_offset;
                stmts.push(quote! { offsets[#i] = #co as u32; });
                stmts.push(quote! {
                    let __vb_len = statevec_model::read_u16_le(data, #co).expect("payload access") as usize;
                    let mut __cursor = #co + 2usize + __vb_len;
                });
                cursor_created = true;
            } else {
                let co = compile_offset;
                stmts.push(quote! { offsets[#i] = #co as u32; });
                compile_offset += f.fixed_size().unwrap();
            }
        } else {
            stmts.push(quote! { offsets[#i] = __cursor as u32; });
            if f.is_var() {
                stmts.push(quote! {
                    let __vb_len = statevec_model::read_u16_le(data, __cursor).expect("payload access") as usize;
                    __cursor += 2usize + __vb_len;
                });
            } else {
                let sz = f.fixed_size().unwrap();
                stmts.push(quote! { __cursor += #sz; });
            }
        }
    }

    stmts
}
