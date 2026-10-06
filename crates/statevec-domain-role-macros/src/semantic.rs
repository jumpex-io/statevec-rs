use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Ident, ItemFn, Path, Token, bracketed, parse_macro_input, parse_quote};

fn kind(ident: &Ident) -> syn::Result<u8> {
    match ident.to_string().as_str() {
        "OwnerPath" => Ok(1),
        "ProductionBoundDst" => Ok(2),
        "PhysicalIt" => Ok(4),
        "CompileFail" => Ok(8),
        _ => Err(syn::Error::new_spanned(ident, "expected OwnerPath, ProductionBoundDst, PhysicalIt or CompileFail")),
    }
}
fn key(input: ParseStream<'_>, expected: &str) -> syn::Result<()> {
    let name: Ident = input.parse()?;
    if name != expected {
        return Err(syn::Error::new_spanned(name, format!("expected {expected}")));
    }
    input.parse::<Token![=]>()?;
    Ok(())
}
struct Contract {
    owner: Path,
    mask: u8,
}
impl Parse for Contract {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        key(input, "owner")?;
        let owner = input.parse()?;
        input.parse::<Token![,]>()?;
        key(input, "requires")?;
        let kinds;
        bracketed!(kinds in input);
        let mut mask = 0;
        for name in kinds.parse_terminated(Ident::parse, Token![,])? {
            let bit = kind(&name)?;
            if mask & bit != 0 {
                return Err(syn::Error::new_spanned(name, "duplicate evidence kind"));
            }
            mask |= bit;
        }
        if mask == 0 {
            return Err(input.error("a contract must require evidence"));
        }
        Ok(Self { owner, mask })
    }
}
struct Evidence {
    transition: syn::Expr,
    kind: u8,
}
impl Parse for Evidence {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        key(input, "transition")?;
        let transition: syn::Expr = input.parse()?;
        if !matches!(transition, syn::Expr::Path(_))
            && !matches!(&transition, syn::Expr::Cast(cast) if matches!(*cast.expr, syn::Expr::Path(_)) && matches!(*cast.ty, syn::Type::BareFn(_)))
        {
            return Err(syn::Error::new_spanned(
                transition,
                "transition must be a function path, optionally coerced to an exact function signature",
            ));
        }
        input.parse::<Token![,]>()?;
        key(input, "kind")?;
        let kind = kind(&input.parse()?)?;
        Ok(Self { transition, kind })
    }
}
pub(super) fn contract(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let Contract { owner, mask } = parse_macro_input!(attribute as Contract);
    super::mark_function(
        item,
        "semantic_contract_v1",
        parse_quote!(
            ::statevec_domain_roles::__semantic_contract_v1_marker::<#owner>(#mask);
        ),
    )
}
pub(super) fn evidence(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let Evidence { transition, kind } = parse_macro_input!(attribute as Evidence);
    let mut function = parse_macro_input!(item as ItemFn);
    if !function.attrs.iter().any(|attr| attr.path().is_ident("test"))
        || function
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("ignore") || attr.path().is_ident("should_panic"))
    {
        return syn::Error::new_spanned(
            &function.sig,
            "semantic_evidence must precede an ordinary, non-ignored #[test], not a helper or expected panic",
        )
        .into_compile_error()
        .into();
    }
    function.block.stmts.insert(
        0,
        parse_quote!(
            ::statevec_domain_roles::__semantic_evidence_marker(#transition, #kind);
        ),
    );
    quote!(#function).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn contract_and_evidence_have_closed_syntax() {
        assert!(syn::parse_str::<Contract>("owner = MyOwner, requires = [OwnerPath, PhysicalIt]").is_ok());
        for bad in [
            "owner = O, requires = []",
            "owner = O, requires = [Smoke]",
            "owner = O, requires = [OwnerPath, OwnerPath]",
        ] {
            assert!(syn::parse_str::<Contract>(bad).is_err());
        }
        assert!(syn::parse_str::<Evidence>("transition = O::step, kind = ProductionBoundDst").is_ok());
        assert!(syn::parse_str::<Evidence>("transition = \"O::step\", kind = OwnerPath").is_err());
    }
}
