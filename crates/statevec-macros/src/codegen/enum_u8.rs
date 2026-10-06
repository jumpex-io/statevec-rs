// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use quote::{format_ident, quote};
use syn::{Attribute, Data, DataEnum, DeriveInput, Expr, Lit, spanned::Spanned};

use crate::codegen::screaming;

pub(crate) fn expand_enum_u8(input: DeriveInput) -> proc_macro2::TokenStream {
    let enum_name = input.ident.clone();

    let mut errors = proc_macro2::TokenStream::new();

    if !has_repr_u8(&input.attrs) {
        errors.extend(syn::Error::new(input.span(), "EnumU8 requires #[repr(u8)]").to_compile_error());
    }

    let Data::Enum(DataEnum { variants, .. }) = input.data else {
        // Can't continue without variants — structural error, return immediately.
        errors.extend(syn::Error::new(input.span(), "EnumU8 only supports enums").to_compile_error());
        return errors;
    };

    let mut match_arms = Vec::new();
    let mut to_u8_arms = Vec::new();
    let mut debug_arms = Vec::new();
    let mut variant_defs = Vec::new();

    for variant in variants.iter() {
        if !variant.fields.is_empty() {
            errors.extend(syn::Error::new(variant.span(), "EnumU8 only supports fieldless enums").to_compile_error());
            continue;
        }

        let Some((_, expr)) = &variant.discriminant else {
            errors.extend(
                syn::Error::new(variant.span(), "EnumU8 requires every variant to have an explicit discriminant")
                    .to_compile_error(),
            );
            continue;
        };

        let Expr::Lit(expr_lit) = expr else {
            errors.extend(
                syn::Error::new(expr.span(), "EnumU8 discriminant must be an integer literal").to_compile_error(),
            );
            continue;
        };

        let Lit::Int(lit_int) = &expr_lit.lit else {
            errors.extend(
                syn::Error::new(expr.span(), "EnumU8 discriminant must be an integer literal").to_compile_error(),
            );
            continue;
        };

        let value = match lit_int.base10_parse::<u16>() {
            Ok(v) if v <= u8::MAX as u16 => v as u8,
            _ => {
                errors.extend(syn::Error::new(lit_int.span(), "EnumU8 discriminant must fit in u8").to_compile_error());
                continue;
            }
        };

        let v_ident = &variant.ident;
        let v_name = v_ident.to_string();

        match_arms.push(quote! {
            #value => ::core::result::Result::Ok(Self::#v_ident)
        });

        to_u8_arms.push(quote! {
            Self::#v_ident => #value
        });
        debug_arms.push(quote! {
            Self::#v_ident => f.write_str(#v_name)
        });

        variant_defs.push(quote! {
            statevec_model::EnumVariantDefinition {
                name: #v_name,
                discriminant: #value,
            }
        });
    }

    if !errors.is_empty() {
        return errors;
    }

    let def_static_name = format_ident!("{}_ENUM_DEFINITION", screaming(&enum_name.to_string()));

    let expanded = quote! {
        static #def_static_name: statevec_model::EnumDefinition = statevec_model::EnumDefinition {
            name: ::core::stringify!(#enum_name),
            variants: &[
                #( #variant_defs, )*
            ],
        };

        impl statevec_model::EnumU8 for #enum_name {
            const DEFINITION: &'static statevec_model::EnumDefinition = &#def_static_name;

            #[inline(always)]
            fn to_u8(self) -> u8 {
                match self {
                    #( #to_u8_arms, )*
                }
            }

            #[inline(always)]
            fn try_from_u8(v: u8) -> ::core::result::Result<Self, statevec_model::EnumDecodeError> {
                match v {
                    #( #match_arms, )*
                    other => ::core::result::Result::Err(statevec_model::EnumDecodeError {
                        type_name: ::core::stringify!(#enum_name),
                        raw: other,
                    }),
                }
            }
        }

        impl From<#enum_name> for u8 {
            #[inline(always)]
            fn from(v: #enum_name) -> Self {
                <#enum_name as statevec_model::EnumU8>::to_u8(v)
            }
        }

        impl ::core::convert::TryFrom<u8> for #enum_name {
            type Error = statevec_model::EnumDecodeError;

            #[inline(always)]
            fn try_from(v: u8) -> Result<Self, Self::Error> {
                <#enum_name as statevec_model::EnumU8>::try_from_u8(v)
            }
        }

        impl ::core::marker::Copy for #enum_name {}

        impl ::core::clone::Clone for #enum_name {
            fn clone(&self) -> Self {
                *self
            }
        }

        impl ::core::fmt::Debug for #enum_name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                match self {
                    #( #debug_arms, )*
                }
            }
        }

        impl ::core::cmp::PartialEq for #enum_name {
            fn eq(&self, other: &Self) -> bool {
                (*self as u8) == (*other as u8)
            }
        }

        impl ::core::cmp::Eq for #enum_name {}

        impl ::core::hash::Hash for #enum_name {
            fn hash<H: ::core::hash::Hasher>(&self, state: &mut H) {
                <#enum_name as statevec_model::EnumU8>::to_u8(*self).hash(state);
            }
        }
    };

    expanded
}

pub(crate) fn has_repr_u8(attrs: &[Attribute]) -> bool {
    for attr in attrs {
        if !attr.path().is_ident("repr") {
            continue;
        }
        let mut found = false;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("u8") {
                found = true;
            }
            Ok(())
        });
        if found {
            return true;
        }
    }
    false
}
