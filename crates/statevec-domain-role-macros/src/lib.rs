use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{ImplItemFn, Item, ItemFn, ItemMod, LitStr, parse_macro_input, parse_quote};

mod semantic;

/// Compiler metadata only: a shipping boundary and its required evidence kinds.
#[proc_macro_attribute]
pub fn semantic_contract_v1(attribute: TokenStream, item: TokenStream) -> TokenStream {
    semantic::contract(attribute, item)
}

/// Binds an exact Rust test to a compiler-resolved production transition.
#[proc_macro_attribute]
pub fn semantic_evidence(attribute: TokenStream, item: TokenStream) -> TokenStream {
    semantic::evidence(attribute, item)
}

#[proc_macro_attribute]
pub fn domain_owner_module(attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_module(attribute, item, "owner")
}

#[proc_macro_attribute]
pub fn domain_adapter_module(attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_module(attribute, item, "adapter")
}

#[proc_macro_attribute]
pub fn domain_port_module(attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_module(attribute, item, "port")
}

#[proc_macro_attribute]
pub fn domain_invariant(attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_named_function(attribute, item, "domain_invariant", "__domain_invariant_marker")
}

#[proc_macro_attribute]
pub fn domain_error_mapper(attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_named_function(attribute, item, "domain_error_mapper", "__domain_error_mapper_marker")
}

fn mark_named_function(
    attribute: TokenStream,
    item: TokenStream,
    attribute_name: &str,
    marker_name: &str,
) -> TokenStream {
    let domain = parse_macro_input!(attribute as LitStr);
    if !valid_domain_identity(&domain.value()) {
        return syn::Error::new_spanned(domain, "domain identity must be a non-empty lowercase snake_case literal")
            .into_compile_error()
            .into();
    }
    let marker = format_ident!("{marker_name}");
    mark_function(item, attribute_name, parse_quote!(::statevec_domain_roles::#marker(#domain);))
}

fn mark_module(attribute: TokenStream, item: TokenStream, role: &str) -> TokenStream {
    let domain = parse_macro_input!(attribute as LitStr);
    let domain_name = domain.value();
    if !valid_domain_identity(&domain_name) {
        return syn::Error::new_spanned(domain, "domain identity must be a non-empty lowercase snake_case literal")
            .into_compile_error()
            .into();
    }
    let module = parse_macro_input!(item as ItemMod);
    let module_name = module.ident.to_string().to_ascii_uppercase();
    let role_name = role.to_ascii_uppercase();
    let domain_marker = domain_name.to_ascii_uppercase();
    let marker = format_ident!("__STATEVEC_DOMAIN_{role_name}__{domain_marker}__MODULE__{module_name}");
    quote! {
        #module
        #[doc(hidden)]
        const #marker: &str = #domain;
    }
    .into()
}

fn valid_domain_identity(identity: &str) -> bool {
    !identity.is_empty()
        && identity.bytes().next().is_some_and(|byte| byte.is_ascii_lowercase())
        && !identity.ends_with('_')
        && !identity.contains("__")
        && identity
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::valid_domain_identity;

    #[test]
    fn domain_identity_is_stable_lowercase_snake_case() {
        for valid in ["raft", "checkpoint_v2", "archive_module_store"] {
            assert!(valid_domain_identity(valid), "expected `{valid}` to be valid");
        }
        for invalid in ["", "2raft", "_raft", "raft_", "raft__log", "Raft", "raft-log"] {
            assert!(!valid_domain_identity(invalid), "expected `{invalid}` to be invalid");
        }
    }
}

#[proc_macro_attribute]
pub fn domain_transition(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_function(item, "domain_transition", parse_quote!(::statevec_domain_roles::__domain_transition_marker();))
}

#[proc_macro_attribute]
pub fn domain_resource_terminal(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_function(
        item,
        "domain_resource_terminal",
        parse_quote!(::statevec_domain_roles::__domain_resource_terminal_marker();),
    )
}

#[proc_macro_attribute]
pub fn domain_initial_owner_creation(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_function(
        item,
        "domain_initial_owner_creation",
        parse_quote!(::statevec_domain_roles::__domain_initial_owner_creation_marker();),
    )
}

#[proc_macro_attribute]
pub fn physical_observation_minter(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_function(
        item,
        "physical_observation_minter",
        parse_quote!(::statevec_domain_roles::__physical_observation_minter_marker();),
    )
}

fn mark_function(item: TokenStream, attribute: &str, marker: syn::Stmt) -> TokenStream {
    let original = item.clone();
    if let Ok(mut function) = syn::parse::<ItemFn>(item.clone()) {
        function.block.stmts.insert(0, marker.clone());
        return quote!(#function).into();
    }
    if let Ok(mut method) = syn::parse::<ImplItemFn>(item) {
        method.block.stmts.insert(0, marker);
        return quote!(#method).into();
    }
    syn::Error::new_spanned(
        proc_macro2::TokenStream::from(original),
        format!("{attribute} requires a function or method"),
    )
    .into_compile_error()
    .into()
}

#[proc_macro_attribute]
pub fn domain_actor(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_role(item, quote!(::statevec_domain_roles::DomainActorRole))
}

#[proc_macro_attribute]
pub fn domain_event(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_role(item, quote!(::statevec_domain_roles::DomainEventRole))
}

#[proc_macro_attribute]
pub fn exclusive_domain_owner(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_role(item, quote!(::statevec_domain_roles::ExclusiveDomainOwnerRole))
}

#[proc_macro_attribute]
pub fn ratcheted_owner(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_role(item, quote!(::statevec_domain_roles::RatchetedOwnerRole))
}

#[proc_macro_attribute]
pub fn physical_worker(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_role(item, quote!(::statevec_domain_roles::PhysicalWorkerRole))
}

#[proc_macro_attribute]
pub fn shipping_adapter(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_role(item, quote!(::statevec_domain_roles::ShippingAdapterRole))
}

#[proc_macro_attribute]
pub fn domain_effect(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_role(item, quote!(::statevec_domain_roles::DomainEffectRole))
}

#[proc_macro_attribute]
pub fn domain_feedback(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_role(item, quote!(::statevec_domain_roles::DomainFeedbackRole))
}

#[proc_macro_attribute]
pub fn domain_failure(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_role(item, quote!(::statevec_domain_roles::DomainFailureRole))
}

#[proc_macro_attribute]
pub fn domain_rejection(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_role(item, quote!(::statevec_domain_roles::DomainRejectionRole))
}

#[proc_macro_attribute]
pub fn domain_result(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_role(item, quote!(::statevec_domain_roles::ClosedDomainResultRole))
}

#[proc_macro_attribute]
pub fn domain_observation(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_role(item, quote!(::statevec_domain_roles::DomainObservationRole))
}

#[proc_macro_attribute]
pub fn domain_projection(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_role(item, quote!(::statevec_domain_roles::DomainProjectionRole))
}

#[proc_macro_attribute]
pub fn domain_core_state(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_role(item, quote!(::statevec_domain_roles::DomainCoreStateRole))
}

/// A shared semantic record, not a lifecycle owner or an observation projection.
#[proc_macro_attribute]
pub fn domain_record(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_role(item, quote!(::statevec_domain_roles::DomainRecordRole))
}

#[proc_macro_attribute]
pub fn stateright_model(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_role(item, quote!(::statevec_domain_roles::StaterightModelRole))
}

#[proc_macro_attribute]
pub fn production_bound_stateright_model(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_role(item, quote!(::statevec_domain_roles::ProductionBoundStaterightModelRole))
}

#[proc_macro_attribute]
pub fn evidence_pending_foundation(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_role(item, quote!(::statevec_domain_roles::EvidencePendingFoundationRole))
}

#[proc_macro_attribute]
pub fn move_only_resource(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    mark_role(item, quote!(::statevec_domain_roles::MoveOnlyResourceRole))
}

fn mark_role(item: TokenStream, role: proc_macro2::TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as Item);
    let (identifier, generics) = match &item {
        Item::Struct(item) => (&item.ident, &item.generics),
        Item::Enum(item) => (&item.ident, &item.generics),
        _ => {
            return syn::Error::new_spanned(item, "domain role attributes require a struct or enum")
                .into_compile_error()
                .into();
        }
    };
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();

    quote! {
        #item
        impl #impl_generics #role for #identifier #type_generics #where_clause {}
    }
    .into()
}
