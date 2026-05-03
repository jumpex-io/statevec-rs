// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use quote::{ToTokens, quote};
use syn::{ExprPath, Ident, Path, Token, Type, parse::Parse, parse::ParseStream};

pub(crate) struct CommandDispatchInput {
    pub fn_name: Ident,
    pub runtime: Path,
    pub error: Type,
    pub entries: Vec<CommandDispatchEntry>,
}

pub(crate) struct CommandDispatchEntry {
    pub command: Path,
    pub handler: ExprPath,
}

impl Parse for CommandDispatchInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        input.parse::<Token![fn]>()?;
        let fn_name: Ident = input.parse()?;
        input.parse::<Token![;]>()?;

        let runtime_kw: Ident = input.parse()?;
        if runtime_kw != Ident::new("runtime", runtime_kw.span()) {
            return Err(syn::Error::new(
                runtime_kw.span(),
                "expected `runtime = Type;`",
            ));
        }
        input.parse::<Token![=]>()?;
        let runtime: Path = input.parse()?;
        input.parse::<Token![;]>()?;

        let error_kw: Ident = input.parse()?;
        if error_kw != Ident::new("error", error_kw.span()) {
            return Err(syn::Error::new(
                error_kw.span(),
                "expected `error = ErrorType;`",
            ));
        }
        input.parse::<Token![=]>()?;
        let error: Type = input.parse()?;
        input.parse::<Token![;]>()?;

        let mut entries = Vec::new();
        while !input.is_empty() {
            entries.push(input.parse()?);
            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }

        if entries.is_empty() {
            return Err(syn::Error::new(
                fn_name.span(),
                "command_dispatch! requires at least one `CommandType => handler` entry",
            ));
        }

        Ok(Self {
            fn_name,
            runtime,
            error,
            entries,
        })
    }
}

impl Parse for CommandDispatchEntry {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let command: Path = input.parse()?;
        input.parse::<Token![=>]>()?;
        let handler: ExprPath = input.parse()?;
        Ok(Self { command, handler })
    }
}

pub(crate) fn expand_command_dispatch(
    input: CommandDispatchInput,
) -> syn::Result<proc_macro2::TokenStream> {
    let fn_name = &input.fn_name;
    let runtime = &input.runtime;
    let error = &input.error;

    // Detect duplicate command type paths at macro expansion time.
    {
        let mut seen = std::collections::HashMap::<String, &Path>::new();
        for entry in &input.entries {
            let key = entry.command.to_token_stream().to_string();
            if let Some(first) = seen.get(&key) {
                let mut err = syn::Error::new_spanned(
                    &entry.command,
                    format!("duplicate command type `{}` in command_dispatch!", key),
                );
                err.combine(syn::Error::new_spanned(first, "first registered here"));
                return Err(err);
            }
            seen.insert(key, &entry.command);
        }
    }

    // Generate pairwise const assertions so that duplicate KIND values produce
    // a compile-time error that names the two conflicting command types.
    let mut kind_assertions = Vec::new();
    for i in 0..input.entries.len() {
        for j in (i + 1)..input.entries.len() {
            let a = &input.entries[i].command;
            let b = &input.entries[j].command;
            kind_assertions.push(quote! {
                const _: () = assert!(
                    <#a as statevec_model::CommandSchema>::KIND
                        != <#b as statevec_model::CommandSchema>::KIND,
                    concat!(
                        "duplicate KIND in command_dispatch!: `",
                        stringify!(#a),
                        "` and `",
                        stringify!(#b),
                        "` resolve to the same kind value"
                    )
                );
            });
        }
    }

    let arms = input.entries.iter().map(|entry| {
        let command = &entry.command;
        let handler = &entry.handler;
        quote! {
            <#command as statevec_model::CommandSchema>::KIND => {
                let command = <#command as statevec_model::GeneratedCommandAccess>::wrap(env.payload());
                #handler(self, tx, command)?;
                ::core::result::Result::Ok(true)
            }
        }
    });

    Ok(quote! {
        #( #kind_assertions )*

        impl #runtime {
            fn #fn_name<Tx: statevec_api::TypedTxContext + ?Sized>(
                &self,
                tx: &mut Tx,
                env: &dyn statevec_api::RuntimeCommandEnvelope,
            ) -> ::core::result::Result<bool, #error>
            where
                #error: ::core::convert::From<<Tx as statevec_api::TypedTxContext>::Error>,
            {
                match env.command_kind() {
                    #( #arms, )*
                    _ => ::core::result::Result::Ok(false),
                }
            }
        }
    })
}
