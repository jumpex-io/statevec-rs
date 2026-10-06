// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use syn::{
    Attribute, Expr, ExprArray, GenericArgument, Ident, Lit, LitInt, LitStr, PathArguments, Type, spanned::Spanned,
};

pub(crate) struct ParsedUniqueKey {
    pub id: u8,
    pub name: Option<String>,
    pub fields: Vec<Ident>,
}

pub(crate) struct ParsedCanonicalIndex {
    pub id: u8,
    pub name: String,
    pub fields: Vec<Ident>,
}

pub(crate) struct RecordArgs {
    pub kind: u16,
    pub record_len: usize,
    pub version: u16,
    pub unique_keys: Vec<ParsedUniqueKey>,
    pub canonical_indexes: Vec<ParsedCanonicalIndex>,
}

pub(crate) fn parse_record_args(args_ts: proc_macro2::TokenStream) -> syn::Result<RecordArgs> {
    let mut kind: Option<u16> = None;
    let mut record_len: Option<usize> = None;
    let mut schema_module_version: Option<u16> = None;
    let mut unique_keys: Vec<ParsedUniqueKey> = Vec::new();
    let mut canonical_indexes: Vec<ParsedCanonicalIndex> = Vec::new();

    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("kind") {
            let lit: LitInt = meta.value()?.parse()?;
            kind = Some(lit.base10_parse::<u16>()?);
            return Ok(());
        }

        if meta.path.is_ident("record_len") {
            let lit: LitInt = meta.value()?.parse()?;
            record_len = Some(lit.base10_parse::<usize>()?);
            return Ok(());
        }

        if meta.path.is_ident("version") {
            return Err(meta.error("individual version is not supported; use #[schema_module(version = \"M.N\")]"));
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
            return Err(meta.error("pk(...) is no longer supported; use uk(id = 0, fields = [...])"));
        }

        if meta.path.is_ident("uk") {
            let mut uk_id: Option<u8> = None;
            let mut uk_name: Option<String> = None;
            let mut field_idents: Option<Vec<Ident>> = None;
            meta.parse_nested_meta(|nested| {
                if nested.path.is_ident("id") {
                    let lit: LitInt = nested.value()?.parse()?;
                    uk_id = Some(lit.base10_parse::<u8>()?);
                    return Ok(());
                }
                if nested.path.is_ident("name") {
                    let lit: LitStr = nested.value()?.parse()?;
                    uk_name = Some(lit.value());
                    return Ok(());
                }
                if nested.path.is_ident("fields") {
                    let value: Expr = nested.value()?.parse()?;
                    let Expr::Array(ExprArray { elems, .. }) = value else {
                        return Err(syn::Error::new(value.span(), "uk(fields = [...]) expected"));
                    };

                    let mut out = Vec::new();
                    for elem in elems {
                        let Expr::Path(p) = elem else {
                            return Err(syn::Error::new(elem.span(), "uk field must be an ident"));
                        };
                        let Some(ident) = p.path.get_ident() else {
                            return Err(syn::Error::new(p.span(), "uk field must be an ident"));
                        };
                        out.push(ident.clone());
                    }
                    field_idents = Some(out);
                    return Ok(());
                }
                Err(syn::Error::new(nested.path.span(), "unsupported uk(...) option"))
            })?;
            let id = uk_id.ok_or_else(|| meta.error("uk(...) requires id = N"))?;
            let fields = field_idents.ok_or_else(|| meta.error("uk(...) requires fields = [...]"))?;
            unique_keys.push(ParsedUniqueKey { id, name: uk_name, fields });
            return Ok(());
        }

        if meta.path.is_ident("index") {
            let mut index_id: Option<u8> = None;
            let mut index_name: Option<String> = None;
            let mut index_fields: Option<Vec<Ident>> = None;
            meta.parse_nested_meta(|nested| {
                if nested.path.is_ident("id") {
                    let lit: LitInt = nested.value()?.parse()?;
                    index_id = Some(lit.base10_parse::<u8>()?);
                    return Ok(());
                }
                if nested.path.is_ident("name") {
                    let lit: LitStr = nested.value()?.parse()?;
                    index_name = Some(lit.value());
                    return Ok(());
                }
                if nested.path.is_ident("fields") {
                    let value: Expr = nested.value()?.parse()?;
                    index_fields = Some(parse_ident_array(value, "index")?);
                    return Ok(());
                }
                Err(syn::Error::new(nested.path.span(), "unsupported index(...) option"))
            })?;
            let id = index_id.ok_or_else(|| meta.error("index(...) requires id = N"))?;
            let name = index_name.ok_or_else(|| meta.error("index(...) requires name = \"...\""))?;
            let fields = index_fields.ok_or_else(|| meta.error("index(...) requires fields = [...]"))?;
            canonical_indexes.push(ParsedCanonicalIndex { id, name, fields });
            return Ok(());
        }

        Err(meta.error("unsupported #[record(...)] argument"))
    });

    syn::parse::Parser::parse2(parser, args_ts)?;

    let args = RecordArgs {
        kind: kind.unwrap_or(0),
        record_len: record_len.unwrap_or(0),
        version: schema_module_version.unwrap_or(1),
        unique_keys,
        canonical_indexes,
    };

    if args.record_len == 0 {
        return Err(syn::Error::new(proc_macro2::Span::call_site(), "record_len must be > 0"));
    }

    if args.unique_keys.len() > statevec_model::MAX_UNIQUE_KEYS_PER_RECORD {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            format!("record supports at most {} UK definitions", statevec_model::MAX_UNIQUE_KEYS_PER_RECORD),
        ));
    }
    let mut uk_ids = std::collections::BTreeSet::new();
    for uk in &args.unique_keys {
        if uk.id as usize >= statevec_model::MAX_UNIQUE_KEYS_PER_RECORD {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                format!(
                    "uk id {} is out of range; valid range is [0, {})",
                    uk.id,
                    statevec_model::MAX_UNIQUE_KEYS_PER_RECORD
                ),
            ));
        }
        if !uk_ids.insert(uk.id) {
            return Err(syn::Error::new(proc_macro2::Span::call_site(), format!("duplicate uk id {}", uk.id)));
        }
    }
    for expected in 0..args.unique_keys.len() {
        if !uk_ids.contains(&(expected as u8)) {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("uk ids must be contiguous from 0; missing id {}", expected),
            ));
        }
    }
    if args.unique_keys.len() > 1 {
        let mut names = std::collections::BTreeSet::new();
        for uk in &args.unique_keys {
            let Some(name) = uk.name.as_ref() else {
                return Err(syn::Error::new(
                    proc_macro2::Span::call_site(),
                    "multiple uk(...) definitions must all declare name = \"...\"",
                ));
            };
            if name.is_empty() {
                return Err(syn::Error::new(proc_macro2::Span::call_site(), "uk name cannot be empty"));
            }
            if syn::parse_str::<Ident>(name).is_err() {
                return Err(syn::Error::new(
                    proc_macro2::Span::call_site(),
                    format!("uk name '{name}' must be a valid Rust identifier"),
                ));
            }
            if !names.insert(name.clone()) {
                return Err(syn::Error::new(proc_macro2::Span::call_site(), format!("duplicate uk name '{name}'")));
            }
        }
    } else if let Some(uk) = args.unique_keys.first()
        && let Some(name) = uk.name.as_ref()
        && syn::parse_str::<Ident>(name).is_err()
    {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            format!("uk name '{name}' must be a valid Rust identifier"),
        ));
    }

    if args.canonical_indexes.len() > statevec_model::MAX_CANONICAL_INDEXES_PER_RECORD {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            format!(
                "record supports at most {} canonical index definitions",
                statevec_model::MAX_CANONICAL_INDEXES_PER_RECORD
            ),
        ));
    }
    let mut index_names = std::collections::BTreeSet::new();
    let mut index_ids = std::collections::BTreeSet::new();
    for index in &args.canonical_indexes {
        if index.id as usize >= statevec_model::MAX_CANONICAL_INDEXES_PER_RECORD {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                format!(
                    "index id {} is out of range; valid range is [0, {})",
                    index.id,
                    statevec_model::MAX_CANONICAL_INDEXES_PER_RECORD
                ),
            ));
        }
        if !index_ids.insert(index.id) {
            return Err(syn::Error::new(proc_macro2::Span::call_site(), format!("duplicate index id {}", index.id)));
        }
        if index.name.is_empty() {
            return Err(syn::Error::new(proc_macro2::Span::call_site(), "index name cannot be empty"));
        }
        if syn::parse_str::<Ident>(&index.name).is_err() {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("index name '{}' must be a valid Rust identifier", index.name),
            ));
        }
        if !index_names.insert(index.name.clone()) {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("duplicate index name '{}'", index.name),
            ));
        }
    }

    for expected in 0..args.canonical_indexes.len() {
        if !index_ids.contains(&(expected as u8)) {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("index ids must be contiguous from 0; missing id {}", expected),
            ));
        }
    }

    Ok(args)
}

fn parse_ident_array(value: Expr, label: &str) -> syn::Result<Vec<Ident>> {
    let Expr::Array(ExprArray { elems, .. }) = value else {
        return Err(syn::Error::new(value.span(), format!("{label}(fields=[...]) expected")));
    };

    let mut out = Vec::new();
    for elem in elems {
        let Expr::Path(p) = elem else {
            return Err(syn::Error::new(elem.span(), format!("{label} field must be an ident")));
        };
        let Some(ident) = p.path.get_ident() else {
            return Err(syn::Error::new(p.span(), format!("{label} field must be an ident")));
        };
        out.push(ident.clone());
    }
    Ok(out)
}

pub(crate) struct ParsedField {
    pub ident: Ident,
    pub index: u32,
    pub ty: Type,
    pub ty_kind: ParsedTypeKind,
    pub fixed_size: usize,
    pub offset: usize,
    pub uk_supported: bool,
    pub immutable: bool,
    pub reserved: bool,
    pub semantic: Option<statevec_model::SemanticTag>,
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
    Decimal { scale: u8 },
}

const MAX_DECIMAL_SCALE: u8 = 38;

fn parse_usize_const_arg(_ty: &Type, last: &syn::PathSegment, type_name: &str) -> syn::Result<usize> {
    let PathArguments::AngleBracketed(args) = &last.arguments else {
        return Err(syn::Error::new(last.arguments.span(), format!("{type_name} must be {type_name}<N>")));
    };
    let Some(GenericArgument::Const(Expr::Lit(expr_lit))) = args.args.first() else {
        return Err(syn::Error::new(args.span(), format!("{type_name} must be {type_name}<N>")));
    };
    let Lit::Int(lit_int) = &expr_lit.lit else {
        return Err(syn::Error::new(expr_lit.span(), format!("{type_name} const param must be integer literal")));
    };
    lit_int.base10_parse::<usize>()
}

fn parse_decimal_scale(ty: &Type, last: &syn::PathSegment) -> syn::Result<u8> {
    let scale = parse_usize_const_arg(ty, last, "Decimal")?;
    let scale = u8::try_from(scale).map_err(|_| syn::Error::new(ty.span(), "Decimal scale must fit in u8"))?;
    if scale > MAX_DECIMAL_SCALE {
        return Err(syn::Error::new(ty.span(), format!("Decimal scale must be <= {MAX_DECIMAL_SCALE}")));
    }
    Ok(scale)
}

pub(crate) fn parse_field(field: &syn::Field) -> syn::Result<ParsedField> {
    let ident = field
        .ident
        .clone()
        .ok_or_else(|| syn::Error::new(field.span(), "#[record] only supports named-field structs"))?;
    let attrs = parse_field_attrs(&field.attrs)?;
    let index = attrs
        .index
        .ok_or_else(|| syn::Error::new(field.span(), "every #[record] field must declare #[field(index = N)]"))?;

    let (ty_kind, fixed_size, uk_supported) = classify_type(&field.ty, attrs.enum_u8)?;
    validate_semantic(&ty_kind, Some(fixed_size as u32), attrs.semantic)?;

    if attrs.reserved && attrs.immutable {
        return Err(syn::Error::new(field.span(), "reserved fields cannot be immutable"));
    }
    if attrs.reserved && attrs.enum_u8 {
        return Err(syn::Error::new(field.span(), "reserved fields cannot use enum_u8"));
    }

    Ok(ParsedField {
        ident,
        index,
        ty: field.ty.clone(),
        ty_kind,
        fixed_size,
        offset: 0,
        uk_supported,
        immutable: attrs.immutable,
        reserved: attrs.reserved,
        semantic: attrs.semantic,
    })
}

pub(crate) struct FieldAttrs {
    pub index: Option<u32>,
    pub immutable: bool,
    pub enum_u8: bool,
    pub reserved: bool,
    pub semantic: Option<statevec_model::SemanticTag>,
}

pub(crate) fn parse_field_attrs(attrs: &[Attribute]) -> syn::Result<FieldAttrs> {
    let mut result = FieldAttrs { index: None, immutable: false, enum_u8: false, reserved: false, semantic: None };
    let mut seen_field_attr = false;
    for attr in attrs {
        if !attr.path().is_ident("field") {
            continue;
        }
        if seen_field_attr {
            return Err(syn::Error::new(attr.span(), "multiple #[field(...)] attributes are not allowed"));
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
            if meta.path.is_ident("semantic") {
                let lit: LitStr = meta.value()?.parse()?;
                let semantic = lit
                    .value()
                    .parse::<statevec_model::SemanticTag>()
                    .map_err(|err| meta.error(err.to_string()))?;
                result.semantic = Some(semantic);
                return Ok(());
            }
            Err(meta.error("unsupported #[field(...)] option"))
        })?;
    }
    Ok(result)
}

fn validate_semantic(
    ty_kind: &ParsedTypeKind,
    fixed_size: Option<u32>,
    semantic: Option<statevec_model::SemanticTag>,
) -> syn::Result<()> {
    statevec_model::validate_semantic_compat(semantic_ty(ty_kind), fixed_size, semantic)
        .map_err(|err| syn::Error::new(proc_macro2::Span::call_site(), err.to_string()))
}

fn semantic_ty(ty_kind: &ParsedTypeKind) -> statevec_model::FieldType {
    match ty_kind {
        ParsedTypeKind::Bool => statevec_model::FieldType::Bool,
        ParsedTypeKind::U8 => statevec_model::FieldType::U8,
        ParsedTypeKind::U16 => statevec_model::FieldType::U16,
        ParsedTypeKind::U32 => statevec_model::FieldType::U32,
        ParsedTypeKind::U64 => statevec_model::FieldType::U64,
        ParsedTypeKind::I32 => statevec_model::FieldType::I32,
        ParsedTypeKind::I64 => statevec_model::FieldType::I64,
        ParsedTypeKind::U128 => statevec_model::FieldType::U128,
        ParsedTypeKind::FixedBytes { .. } => statevec_model::FieldType::FixedBytes,
        ParsedTypeKind::VarBytes => statevec_model::FieldType::VarBytes,
        ParsedTypeKind::EnumU8 => statevec_model::FieldType::EnumU8,
        ParsedTypeKind::Decimal { .. } => statevec_model::FieldType::Decimal,
    }
}

pub(crate) fn classify_type(ty: &Type, explicit_enum_u8: bool) -> syn::Result<(ParsedTypeKind, usize, bool)> {
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
                let n = parse_usize_const_arg(ty, last, "FixedBytes")?;
                return Ok((ParsedTypeKind::FixedBytes { n }, 2 + n, true));
            }
            "Decimal" => return Ok((ParsedTypeKind::Decimal { scale: parse_decimal_scale(ty, last)? }, 16, false)),
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
                return Err(syn::Error::new(ty.span(), "unknown field type; use #[field(enum_u8)] for enum types"));
            }
        }
    }

    Err(syn::Error::new(ty.span(), "unsupported field type in #[record] v1"))
}

pub(crate) fn classify_payload_type(ty: &Type, explicit_enum_u8: bool) -> syn::Result<ParsedTypeKind> {
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
            "FixedBytes" => Ok(ParsedTypeKind::FixedBytes { n: parse_usize_const_arg(ty, last, "FixedBytes")? }),
            "Decimal" => Ok(ParsedTypeKind::Decimal { scale: parse_decimal_scale(ty, last)? }),
            "VarBytes" => Ok(ParsedTypeKind::VarBytes),
            _ => {
                if explicit_enum_u8 {
                    Ok(ParsedTypeKind::EnumU8)
                } else {
                    Err(syn::Error::new(
                        ty.span(),
                        "unknown field type; supported: bool, u8, u16, u32, u64, i32, i64, u128, FixedBytes<N>, Decimal<N>, VarBytes, or use #[field(enum_u8)]",
                    ))
                }
            }
        };
    }
    Err(syn::Error::new(ty.span(), "unsupported field type in payload macro"))
}

pub(crate) struct PayloadField {
    pub ident: Ident,
    pub index: u32,
    pub ty: Type,
    pub ty_kind: ParsedTypeKind,
    pub semantic: Option<statevec_model::SemanticTag>,
    pub repeated: bool,
    pub element_count_max: u16,
}

impl PayloadField {
    pub fn is_var(&self) -> bool {
        self.repeated || matches!(self.ty_kind, ParsedTypeKind::VarBytes)
    }

    pub fn fixed_size(&self) -> Option<usize> {
        fixed_size_for_type_kind(&self.ty_kind)
    }
}

fn fixed_size_for_type_kind(ty_kind: &ParsedTypeKind) -> Option<usize> {
    match ty_kind {
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
        ParsedTypeKind::Decimal { .. } => Some(16),
        ParsedTypeKind::VarBytes => None,
    }
}

pub(crate) struct PayloadArgs {
    pub kind: u16,
    pub version: u16,
    pub inline_response: Option<bool>,
}

pub(crate) fn parse_payload_args(args_ts: proc_macro2::TokenStream) -> syn::Result<PayloadArgs> {
    let mut kind: Option<u16> = None;
    let mut schema_module_version: Option<u16> = None;
    let mut inline_response: Option<bool> = None;

    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("kind") {
            let lit: LitInt = meta.value()?.parse()?;
            let k = lit.base10_parse::<u16>()?;
            if k == 0 || k > statevec_model::USER_KIND_MAX {
                return Err(syn::Error::new(lit.span(), "kind must be in [1, 61439]"));
            }
            kind = Some(k);
            return Ok(());
        }

        if meta.path.is_ident("inline_response") {
            let lit: syn::LitBool = meta.value()?.parse()?;
            inline_response = Some(lit.value);
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
        Err(meta.error("unsupported argument; expected kind = N or inline_response = true"))
    });

    syn::parse::Parser::parse2(parser, args_ts)?;

    let kind =
        kind.ok_or_else(|| syn::Error::new(proc_macro2::Span::call_site(), "missing required argument: kind = N"))?;

    Ok(PayloadArgs { kind, version: schema_module_version.unwrap_or(1), inline_response })
}

pub(crate) fn parse_payload_field(field: &syn::Field, kind: PayloadKind) -> syn::Result<PayloadField> {
    let ident = field
        .ident
        .clone()
        .ok_or_else(|| syn::Error::new(field.span(), "payload macros only support named-field structs"))?;
    let mut index_opt: Option<u32> = None;
    let mut enum_u8 = false;
    let mut semantic = None;
    let mut repeated = false;
    let mut max = None;
    let mut seen_field_attr = false;

    for attr in &field.attrs {
        if !attr.path().is_ident("field") {
            continue;
        }
        if seen_field_attr {
            return Err(syn::Error::new(attr.span(), "multiple #[field(...)] attributes are not allowed"));
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
            if meta.path.is_ident("semantic") {
                let lit: LitStr = meta.value()?.parse()?;
                semantic = Some(
                    lit.value()
                        .parse::<statevec_model::SemanticTag>()
                        .map_err(|err| meta.error(err.to_string()))?,
                );
                return Ok(());
            }
            if meta.path.is_ident("repeated") {
                repeated = true;
                return Ok(());
            }
            if meta.path.is_ident("max") {
                let lit: LitInt = meta.value()?.parse()?;
                max = Some(lit.base10_parse::<u16>()?);
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

    let index = index_opt
        .ok_or_else(|| syn::Error::new(field.span(), "every payload field must declare #[field(index = N)]"))?;

    if repeated && matches!(kind, PayloadKind::Command) {
        return Err(syn::Error::new(field.span(), "repeated fields are not supported in #[command] payloads"));
    }
    if max.is_some() && !repeated {
        return Err(syn::Error::new(field.span(), "#[field(max = N)] requires #[field(repeated)]"));
    }

    let (ty, ty_kind) = if repeated {
        let element_ty = repeated_element_type(&field.ty)?;
        (element_ty.clone(), classify_payload_type(element_ty, enum_u8)?)
    } else {
        (field.ty.clone(), classify_payload_type(&field.ty, enum_u8)?)
    };
    let fixed_size = fixed_size_for_type_kind(&ty_kind);
    validate_semantic(&ty_kind, fixed_size.map(|n| n as u32), semantic)?;
    let element_count_max = if repeated {
        let max = max.ok_or_else(|| syn::Error::new(field.span(), "repeated fields must declare #[field(max = N)]"))?;
        statevec_model::validate_repeated_payload_compat(semantic_ty(&ty_kind), true, max, false)
            .map_err(|err| syn::Error::new(field.span(), err.to_string()))?;
        max
    } else {
        0
    };

    Ok(PayloadField { ident, index, ty, ty_kind, semantic, repeated, element_count_max })
}

fn repeated_element_type(ty: &Type) -> syn::Result<&Type> {
    let Type::Path(tp) = ty else {
        return Err(syn::Error::new(ty.span(), "repeated payload fields must be Vec<T> or List<T>"));
    };
    let Some(segment) = tp.path.segments.last() else {
        return Err(syn::Error::new(ty.span(), "repeated payload fields must be Vec<T> or List<T>"));
    };
    let ident = segment.ident.to_string();
    if ident != "Vec" && ident != "List" {
        return Err(syn::Error::new(ty.span(), "repeated payload fields must be Vec<T> or List<T>"));
    }
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return Err(syn::Error::new(ty.span(), "repeated payload fields must include one element type"));
    };
    if args.args.len() != 1 {
        return Err(syn::Error::new(ty.span(), "repeated payload fields must include one element type"));
    }
    let Some(syn::GenericArgument::Type(element_ty)) = args.args.first() else {
        return Err(syn::Error::new(ty.span(), "repeated payload fields must include one element type"));
    };
    Ok(element_ty)
}

#[cfg(test)]
#[path = "field_parser/ut_field_parser.rs"]
mod ut_field_parser;

#[derive(Clone, Copy)]
pub(crate) enum PayloadKind {
    Command,
    Event,
}
