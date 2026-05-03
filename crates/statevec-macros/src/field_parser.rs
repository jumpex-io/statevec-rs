// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use syn::{
    Attribute, Expr, ExprArray, GenericArgument, Ident, Lit, LitInt, PathArguments, Type,
    spanned::Spanned,
};

pub(crate) struct RecordArgs {
    pub kind: u8,
    pub record_len: usize,
    pub version: u16,
    pub pk_fields: Vec<Ident>,
}

pub(crate) fn parse_record_args(args_ts: proc_macro2::TokenStream) -> syn::Result<RecordArgs> {
    let mut kind: Option<u8> = None;
    let mut record_len: Option<usize> = None;
    let mut schema_module_version: Option<u16> = None;
    let mut pk_fields: Vec<Ident> = Vec::new();

    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("kind") {
            let lit: LitInt = meta.value()?.parse()?;
            kind = Some(lit.base10_parse::<u8>()?);
            return Ok(());
        }

        if meta.path.is_ident("record_len") {
            let lit: LitInt = meta.value()?.parse()?;
            record_len = Some(lit.base10_parse::<usize>()?);
            return Ok(());
        }

        if meta.path.is_ident("version") {
            return Err(meta.error(
                "individual version is not supported; use #[schema_module(version = \"M.N\")]",
            ));
        }

        if meta.path.is_ident("__schema_module_version") {
            let lit: LitInt = meta.value()?.parse()?;
            let version = lit.base10_parse::<u16>()?;
            if version == 0 {
                return Err(meta.error("__schema_module_version must be >= 1"));
            }
            schema_module_version = Some(version);
            return Ok(());
        }

        if meta.path.is_ident("pk") {
            meta.parse_nested_meta(|nested| {
                if nested.path.is_ident("fields") {
                    let value: Expr = nested.value()?.parse()?;
                    let Expr::Array(ExprArray { elems, .. }) = value else {
                        return Err(syn::Error::new(value.span(), "pk(fields=[...]) expected"));
                    };

                    let mut out = Vec::new();
                    for elem in elems {
                        let Expr::Path(p) = elem else {
                            return Err(syn::Error::new(elem.span(), "pk field must be an ident"));
                        };
                        let Some(ident) = p.path.get_ident() else {
                            return Err(syn::Error::new(p.span(), "pk field must be an ident"));
                        };
                        out.push(ident.clone());
                    }
                    pk_fields = out;
                    return Ok(());
                }
                Err(syn::Error::new(
                    nested.path.span(),
                    "unsupported pk(...) option",
                ))
            })?;
            return Ok(());
        }

        Err(meta.error("unsupported #[record(...)] argument"))
    });

    syn::parse::Parser::parse2(parser, args_ts)?;

    let args = RecordArgs {
        kind: kind.unwrap_or(0),
        record_len: record_len.unwrap_or(0),
        version: schema_module_version.unwrap_or(1),
        pk_fields,
    };

    if args.record_len == 0 {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "record_len must be > 0",
        ));
    }

    Ok(args)
}

pub(crate) struct ParsedField {
    pub ident: Ident,
    pub index: u32,
    pub ty: Type,
    pub ty_kind: ParsedTypeKind,
    pub fixed_size: usize,
    pub offset: usize,
    pub pk_supported: bool,
    pub immutable: bool,
    pub reserved: bool,
}

pub(crate) enum ParsedTypeKind {
    Bool,
    U8,
    U16,
    U32,
    U64,
    I32,
    I64,
    U128,
    FixedBytes { n: usize },
    VarBytes,
    EnumU8,
}

pub(crate) fn parse_field(field: &syn::Field) -> syn::Result<ParsedField> {
    let ident = field.ident.clone().ok_or_else(|| {
        syn::Error::new(field.span(), "#[record] only supports named-field structs")
    })?;
    let attrs = parse_field_attrs(&field.attrs)?;
    let index = attrs.index.ok_or_else(|| {
        syn::Error::new(
            field.span(),
            "every #[record] field must declare #[field(index = N)]",
        )
    })?;

    let (ty_kind, fixed_size, pk_supported) = classify_type(&field.ty, attrs.enum_u8)?;

    if attrs.reserved && attrs.immutable {
        return Err(syn::Error::new(
            field.span(),
            "reserved fields cannot be immutable",
        ));
    }
    if attrs.reserved && attrs.enum_u8 {
        return Err(syn::Error::new(
            field.span(),
            "reserved fields cannot use enum_u8",
        ));
    }

    Ok(ParsedField {
        ident,
        index,
        ty: field.ty.clone(),
        ty_kind,
        fixed_size,
        offset: 0,
        pk_supported,
        immutable: attrs.immutable,
        reserved: attrs.reserved,
    })
}

pub(crate) struct FieldAttrs {
    pub index: Option<u32>,
    pub immutable: bool,
    pub enum_u8: bool,
    pub reserved: bool,
}

pub(crate) fn parse_field_attrs(attrs: &[Attribute]) -> syn::Result<FieldAttrs> {
    let mut result = FieldAttrs {
        index: None,
        immutable: false,
        enum_u8: false,
        reserved: false,
    };
    let mut seen_field_attr = false;
    for attr in attrs {
        if !attr.path().is_ident("field") {
            continue;
        }
        if seen_field_attr {
            return Err(syn::Error::new(
                attr.span(),
                "multiple #[field(...)] attributes are not allowed",
            ));
        }
        seen_field_attr = true;
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("index") {
                let lit: LitInt = meta.value()?.parse()?;
                result.index = Some(lit.base10_parse::<u32>()?);
                return Ok(());
            }
            if meta.path.is_ident("immutable") {
                if meta.input.peek(syn::Token![=]) {
                    let lit: syn::LitBool = meta.value()?.parse()?;
                    result.immutable = lit.value;
                } else {
                    result.immutable = true;
                }
                return Ok(());
            }
            if meta.path.is_ident("enum_u8") {
                result.enum_u8 = true;
                return Ok(());
            }
            if meta.path.is_ident("reserved") {
                result.reserved = true;
                return Ok(());
            }
            Err(meta.error("unsupported #[field(...)] option"))
        })?;
    }
    Ok(result)
}

pub(crate) fn classify_type(
    ty: &Type,
    explicit_enum_u8: bool,
) -> syn::Result<(ParsedTypeKind, usize, bool)> {
    if let Type::Path(tp) = ty {
        let path = &tp.path;
        let Some(last) = path.segments.last() else {
            return Err(syn::Error::new(ty.span(), "empty type path"));
        };

        match last.ident.to_string().as_str() {
            "bool" => return Ok((ParsedTypeKind::Bool, 1, true)),
            "u8" => return Ok((ParsedTypeKind::U8, 1, true)),
            "u16" => return Ok((ParsedTypeKind::U16, 2, true)),
            "u32" => return Ok((ParsedTypeKind::U32, 4, true)),
            "u64" => return Ok((ParsedTypeKind::U64, 8, true)),
            "i32" => return Ok((ParsedTypeKind::I32, 4, true)),
            "i64" => return Ok((ParsedTypeKind::I64, 8, true)),
            "u128" => return Ok((ParsedTypeKind::U128, 16, false)),
            "FixedBytes" => {
                let PathArguments::AngleBracketed(args) = &last.arguments else {
                    return Err(syn::Error::new(
                        last.arguments.span(),
                        "FixedBytes must be FixedBytes<N>",
                    ));
                };
                let Some(GenericArgument::Const(Expr::Lit(expr_lit))) = args.args.first() else {
                    return Err(syn::Error::new(
                        args.span(),
                        "FixedBytes must be FixedBytes<N>",
                    ));
                };
                let Lit::Int(lit_int) = &expr_lit.lit else {
                    return Err(syn::Error::new(
                        expr_lit.span(),
                        "FixedBytes const param must be integer literal",
                    ));
                };
                let n = lit_int.base10_parse::<usize>()?;
                return Ok((ParsedTypeKind::FixedBytes { n }, 2 + n, true));
            }
            "VarBytes" => {
                return Err(syn::Error::new(
                    ty.span(),
                    "VarBytes is not supported in #[record]; use it in #[command] or #[event] only",
                ));
            }
            _ => {
                if explicit_enum_u8 {
                    return Ok((ParsedTypeKind::EnumU8, 1, true));
                }
                return Err(syn::Error::new(
                    ty.span(),
                    "unknown field type; use #[field(enum_u8)] for enum types",
                ));
            }
        }
    }

    Err(syn::Error::new(
        ty.span(),
        "unsupported field type in #[record] v1",
    ))
}

pub(crate) fn classify_payload_type(
    ty: &Type,
    explicit_enum_u8: bool,
) -> syn::Result<ParsedTypeKind> {
    if let Type::Path(tp) = ty {
        let Some(last) = tp.path.segments.last() else {
            return Err(syn::Error::new(ty.span(), "empty type path"));
        };
        return match last.ident.to_string().as_str() {
            "bool" => Ok(ParsedTypeKind::Bool),
            "u8" => Ok(ParsedTypeKind::U8),
            "u16" => Ok(ParsedTypeKind::U16),
            "u32" => Ok(ParsedTypeKind::U32),
            "u64" => Ok(ParsedTypeKind::U64),
            "i32" => Ok(ParsedTypeKind::I32),
            "i64" => Ok(ParsedTypeKind::I64),
            "u128" => Ok(ParsedTypeKind::U128),
            "FixedBytes" => {
                let PathArguments::AngleBracketed(args) = &last.arguments else {
                    return Err(syn::Error::new(
                        ty.span(),
                        "FixedBytes must be FixedBytes<N>",
                    ));
                };
                let Some(GenericArgument::Const(Expr::Lit(expr_lit))) = args.args.first() else {
                    return Err(syn::Error::new(
                        ty.span(),
                        "FixedBytes must be FixedBytes<N>",
                    ));
                };
                let Lit::Int(lit_int) = &expr_lit.lit else {
                    return Err(syn::Error::new(
                        ty.span(),
                        "FixedBytes const param must be integer literal",
                    ));
                };
                Ok(ParsedTypeKind::FixedBytes {
                    n: lit_int.base10_parse::<usize>()?,
                })
            }
            "VarBytes" => Ok(ParsedTypeKind::VarBytes),
            _ => {
                if explicit_enum_u8 {
                    Ok(ParsedTypeKind::EnumU8)
                } else {
                    Err(syn::Error::new(
                        ty.span(),
                        "unknown field type; supported: bool, u8, u16, u32, u64, i32, i64, u128, FixedBytes<N>, VarBytes, or use #[field(enum_u8)]",
                    ))
                }
            }
        };
    }
    Err(syn::Error::new(
        ty.span(),
        "unsupported field type in payload macro",
    ))
}

pub(crate) struct PayloadField {
    pub ident: Ident,
    pub index: u32,
    pub ty: Type,
    pub ty_kind: ParsedTypeKind,
}

impl PayloadField {
    pub fn is_var(&self) -> bool {
        matches!(self.ty_kind, ParsedTypeKind::VarBytes)
    }

    pub fn fixed_size(&self) -> Option<usize> {
        match &self.ty_kind {
            ParsedTypeKind::Bool => Some(1),
            ParsedTypeKind::U8 => Some(1),
            ParsedTypeKind::U16 => Some(2),
            ParsedTypeKind::U32 => Some(4),
            ParsedTypeKind::U64 => Some(8),
            ParsedTypeKind::I32 => Some(4),
            ParsedTypeKind::I64 => Some(8),
            ParsedTypeKind::U128 => Some(16),
            ParsedTypeKind::FixedBytes { n } => Some(2 + n),
            ParsedTypeKind::EnumU8 => Some(1),
            ParsedTypeKind::VarBytes => None,
        }
    }
}

pub(crate) struct PayloadArgs {
    pub kind: u8,
    pub version: u16,
}

pub(crate) fn parse_payload_args(args_ts: proc_macro2::TokenStream) -> syn::Result<PayloadArgs> {
    let mut kind: Option<u8> = None;
    let mut schema_module_version: Option<u16> = None;

    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("kind") {
            let lit: LitInt = meta.value()?.parse()?;
            let k = lit.base10_parse::<u16>()?;
            if k == 0 || k > 255 {
                return Err(syn::Error::new(lit.span(), "kind must be in [1, 255]"));
            }
            kind = Some(k as u8);
            return Ok(());
        }

        if meta.path.is_ident("__schema_module_version") {
            let lit: LitInt = meta.value()?.parse()?;
            let parsed_version = lit.base10_parse::<u16>()?;
            if parsed_version == 0 {
                return Err(meta.error("__schema_module_version must be >= 1"));
            }
            schema_module_version = Some(parsed_version);
            return Ok(());
        }
        Err(meta.error("unsupported argument; expected kind = N"))
    });

    syn::parse::Parser::parse2(parser, args_ts)?;

    let kind = kind.ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            "missing required argument: kind = N",
        )
    })?;

    Ok(PayloadArgs {
        kind,
        version: schema_module_version.unwrap_or(1),
    })
}

pub(crate) fn parse_payload_field(field: &syn::Field) -> syn::Result<PayloadField> {
    let ident = field.ident.clone().ok_or_else(|| {
        syn::Error::new(
            field.span(),
            "payload macros only support named-field structs",
        )
    })?;
    let mut index_opt: Option<u32> = None;
    let mut enum_u8 = false;
    let mut seen_field_attr = false;

    for attr in &field.attrs {
        if !attr.path().is_ident("field") {
            continue;
        }
        if seen_field_attr {
            return Err(syn::Error::new(
                attr.span(),
                "multiple #[field(...)] attributes are not allowed",
            ));
        }
        seen_field_attr = true;
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("index") {
                let lit: LitInt = meta.value()?.parse()?;
                index_opt = Some(lit.base10_parse::<u32>()?);
                return Ok(());
            }
            if meta.path.is_ident("enum_u8") {
                enum_u8 = true;
                return Ok(());
            }
            if meta.path.is_ident("immutable") {
                return Err(syn::Error::new(
                    meta.path.span(),
                    "#[field(immutable)] is not supported in #[command]/#[event] macros",
                ));
            }
            Err(meta.error("unsupported #[field(...)] option"))
        })?;
    }

    let index = index_opt.ok_or_else(|| {
        syn::Error::new(
            field.span(),
            "every payload field must declare #[field(index = N)]",
        )
    })?;

    let ty_kind = classify_payload_type(&field.ty, enum_u8)?;

    Ok(PayloadField {
        ident,
        index,
        ty: field.ty.clone(),
        ty_kind,
    })
}

#[cfg(test)]
#[path = "field_parser/ut_field_parser.rs"]
mod ut_field_parser;

pub(crate) enum PayloadKind {
    Command,
    Event,
}
