// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Procedural macros for StateVec schema-first domain definitions.
//!
//! Most users import these macros through `statevec::prelude::*`.
//! They generate schema metadata, typed accessors, builders, and runtime plugin
//! ABI entrypoints from Rust domain types.

extern crate proc_macro;

mod codegen;
mod field_parser;

use proc_macro::TokenStream;
use syn::{DeriveInput, ItemMod, parse_macro_input};

use crate::codegen::command_dispatch::{CommandDispatchInput, expand_command_dispatch};
use crate::codegen::enum_u8::expand_enum_u8;
use crate::codegen::export_plugin::expand_export_runtime_plugin;
use crate::codegen::payload::expand_payload;
use crate::codegen::record::expand_record;
use crate::codegen::schema_module::{SchemaModuleArgs, expand_schema_module};
use crate::field_parser::PayloadKind;

/// Derives StateVec encoding metadata for a `#[repr(u8)]` enum.
///
/// The generated implementation provides `EnumU8` conversion methods and a
/// static enum definition used by schema registries and IDL output.
#[proc_macro_derive(EnumU8)]
pub fn derive_enum_u8(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand_enum_u8(input).into()
}

/// Defines a fixed-layout StateVec record.
///
/// Required arguments include `kind = <u16>` and `record_len = <usize>`.
/// `record_len` includes the header and must be a power of two from 64 bytes
/// through the runtime's 32 KiB limit ([`statevec_model::MAX_RECORD_LEN`]).
/// Optional unique-key metadata can be provided with
/// `uk(id = 0, fields = [field_a, field_b])`.
#[proc_macro_attribute]
pub fn record(args: TokenStream, input: TokenStream) -> TokenStream {
    let args_ts: proc_macro2::TokenStream = args.into();
    let input_ast = parse_macro_input!(input as DeriveInput);

    match expand_record(args_ts, input_ast) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// Defines a StateVec command payload type.
///
/// The macro generates a command schema definition, encoded payload builder,
/// and typed read accessor.
#[proc_macro_attribute]
pub fn command(args: TokenStream, input: TokenStream) -> TokenStream {
    let args_ts: proc_macro2::TokenStream = args.into();
    let input_ast = parse_macro_input!(input as DeriveInput);
    match expand_payload(args_ts, input_ast, PayloadKind::Command) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// Defines a StateVec event payload type.
///
/// The macro generates an event schema definition, encoded payload builder, and
/// typed read accessor.
#[proc_macro_attribute]
pub fn event(args: TokenStream, input: TokenStream) -> TokenStream {
    let args_ts: proc_macro2::TokenStream = args.into();
    let input_ast = parse_macro_input!(input as DeriveInput);
    match expand_payload(args_ts, input_ast, PayloadKind::Event) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// Defines a schema module and generates registry/identity helpers.
///
/// The enclosing module version is provided as `version = "MAIN.MINOR"`.
/// Nested `#[record]`, `#[command]`, `#[event]`, and `EnumU8` items are
/// collected into a `SchemaRegistry`.
#[proc_macro_attribute]
pub fn schema_module(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as SchemaModuleArgs);
    let input = parse_macro_input!(input as ItemMod);
    expand_schema_module(args, input)
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

/// Exports a runtime plugin factory through the stable StateVec plugin ABI.
///
/// The input expression must evaluate to `Box<dyn RuntimePluginFactory>`.
#[proc_macro]
pub fn export_runtime_plugin(input: TokenStream) -> TokenStream {
    let factory_expr = parse_macro_input!(input as syn::Expr);
    expand_export_runtime_plugin(factory_expr).into()
}

/// Generates command-kind dispatch code for runtime plugins.
///
/// This macro is intended for plugin implementations that need to route
/// `RuntimeCommandEnvelope` values to generated command accessors.
#[proc_macro]
pub fn command_dispatch(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as CommandDispatchInput);
    expand_command_dispatch(input).unwrap_or_else(|e| e.to_compile_error()).into()
}
