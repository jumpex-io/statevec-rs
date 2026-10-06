//! Fallible checking generated from the same fields as command accessors.
use crate::field_parser::{ParsedTypeKind, PayloadField};
use proc_macro2::TokenStream;
use quote::quote;

pub(super) fn validate_payload(fields: &[PayloadField]) -> TokenStream {
    let checks = fields.iter().map(|field| {
        let index = field.index;
        let scalar = scalar(field);
        // parse_payload_field rejects repeated command fields.
        if matches!(field.ty_kind, ParsedTypeKind::VarBytes) {
            quote! {
                let (_, next) = statevec_model::read_var_bytes(data, cursor)
                    .map_err(|source| statevec_model::CommandSchemaFailure::FieldAccess { field: #index, source })?;
                cursor = next;
            }
        } else {
            scalar
        }
    });
    quote! {
        fn validate_payload(data: &[u8]) -> ::core::result::Result<(), statevec_model::CommandSchemaFailure> {
            let mut cursor = 0usize;
            #( #checks )*
            if cursor != data.len() {
                return ::core::result::Result::Err(statevec_model::CommandSchemaFailure::TrailingBytes { consumed: cursor, actual: data.len() });
            }
            ::core::result::Result::Ok(())
        }
    }
}

fn scalar(field: &PayloadField) -> TokenStream {
    let index = field.index;
    let ty = &field.ty;
    let size = match &field.ty_kind {
        ParsedTypeKind::Bool | ParsedTypeKind::U8 | ParsedTypeKind::EnumU8 => 1,
        ParsedTypeKind::U16 => 2,
        ParsedTypeKind::U32 | ParsedTypeKind::I32 => 4,
        ParsedTypeKind::U64 | ParsedTypeKind::I64 => 8,
        ParsedTypeKind::U128 | ParsedTypeKind::Decimal { .. } => 16,
        ParsedTypeKind::FixedBytes { n } => 2 + n,
        ParsedTypeKind::VarBytes => return quote! {}, // checked by its own length reader
    };
    let value = match &field.ty_kind {
        ParsedTypeKind::Bool => quote! {
            let raw = data[cursor];
            if raw > 1 { return ::core::result::Result::Err(statevec_model::CommandSchemaFailure::Boolean { field: #index, raw }); }
        },
        ParsedTypeKind::EnumU8 => {
            quote! {
                <#ty as statevec_model::EnumU8>::try_from_u8(data[cursor])
                    .map_err(|source| statevec_model::CommandSchemaFailure::Enum { field: #index, source })?;
            }
        }
        ParsedTypeKind::FixedBytes { n } => quote! {
            let length = usize::from(u16::from_le_bytes([data[cursor], data[cursor + 1]]));
            if length > #n {
                return ::core::result::Result::Err(statevec_model::CommandSchemaFailure::FixedBytesLength { field: #index, length, capacity: #n });
            }
            let padding_start = cursor + 2 + length;
            if let ::core::option::Option::Some(offset) = data[padding_start..end].iter().position(|byte| *byte != 0) {
                return ::core::result::Result::Err(statevec_model::CommandSchemaFailure::FixedBytesPadding {
                    field: #index, offset: length + offset, raw: data[padding_start + offset],
                });
            }
        },
        _ => quote! {},
    };
    quote! {
        {
            let end = cursor.checked_add(#size).ok_or_else(|| statevec_model::CommandSchemaFailure::FieldAccess {
                field: #index, source: statevec_model::AccessError { required: usize::MAX, actual: data.len() },
            })?;
            if end > data.len() {
                return ::core::result::Result::Err(statevec_model::CommandSchemaFailure::FieldAccess {
                    field: #index, source: statevec_model::AccessError { required: end, actual: data.len() },
                });
            }
            #value
            cursor = end;
        }
    }
}
