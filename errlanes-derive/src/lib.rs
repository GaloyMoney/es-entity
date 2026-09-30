//! Proc macros for `errlanes`: `#[derive(Rejection)]`, `#[derive(Lift)]`, `#[derive(Failure)]`,
//! and `#[compose]`.

mod composition;
mod failure;
mod lift;
mod rejection;

use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

// Resolve the runtime where the macro is invoked, including dependency aliases
// and EsRepo consumers that use es_entity's reexport without a direct dependency.
fn runtime_path() -> proc_macro2::TokenStream {
    use proc_macro_crate::{FoundCrate, crate_name};
    match crate_name("errlanes") {
        Ok(FoundCrate::Name(name)) => {
            let name = quote::format_ident!("{name}");
            quote::quote!(#name)
        }
        Ok(FoundCrate::Itself) => quote::quote!(errlanes),
        Err(_) => match crate_name("es-entity") {
            Ok(FoundCrate::Name(name)) => {
                let name = quote::format_ident!("{name}");
                quote::quote!(#name::errlanes)
            }
            Ok(FoundCrate::Itself) => quote::quote!(es_entity::errlanes),
            Err(_) => quote::quote!(errlanes),
        },
    }
}

fn resolve_runtime(tokens: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    use proc_macro2::{Group, TokenTree};
    let mut output = proc_macro2::TokenStream::new();
    let mut input = tokens.into_iter().peekable();
    let mut qualified = false;
    while let Some(token) = input.next() {
        let was_qualified = qualified;
        qualified = matches!(&token, TokenTree::Punct(p) if p.as_char() == ':');
        match token {
            TokenTree::Ident(ref name)
                if name == "errlanes"
                    && !was_qualified
                    && matches!(input.peek(), Some(TokenTree::Punct(p)) if p.as_char() == ':') =>
            {
                output.extend(runtime_path());
            }
            TokenTree::Group(group) => {
                let mut rewritten = Group::new(group.delimiter(), resolve_runtime(group.stream()));
                rewritten.set_span(group.span());
                output.extend([TokenTree::Group(rewritten)]);
            }
            other => output.extend([other]),
        }
    }
    output
}

fn expand(
    input: TokenStream,
    f: impl FnOnce(&DeriveInput) -> darling::Result<proc_macro2::TokenStream>,
) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    match f(&ast) {
        Ok(tokens) => resolve_runtime(tokens).into(),
        Err(e) => e.write_errors().into(),
    }
}

#[proc_macro_derive(Rejection, attributes(rejection, lift))]
pub fn derive_rejection(input: TokenStream) -> TokenStream {
    expand(input, rejection::derive)
}

#[proc_macro_derive(Lift, attributes(lift))]
pub fn derive_lift(input: TokenStream) -> TokenStream {
    expand(input, |ast| lift::derive(ast).map_err(darling::Error::from))
}

#[proc_macro_derive(Failure, attributes(failure))]
pub fn derive_failure(input: TokenStream) -> TokenStream {
    expand(input, failure::derive)
}

#[proc_macro_attribute]
pub fn compose(args: TokenStream, input: TokenStream) -> TokenStream {
    if !args.is_empty() {
        return syn::Error::new(
            proc_macro2::Span::call_site(),
            "compose takes no arguments; use #[compose(flatten)] on source placeholders",
        )
        .to_compile_error()
        .into();
    }
    match syn::parse::<syn::ItemEnum>(input).and_then(composition::expand) {
        Ok(t) => resolve_runtime(t).into(),
        Err(e) => e.to_compile_error().into(),
    }
}

#[doc(hidden)]
#[proc_macro]
pub fn __compose_rejection(input: TokenStream) -> TokenStream {
    match composition::callback(input.into()) {
        Ok(t) => resolve_runtime(t).into(),
        Err(e) => e.to_compile_error().into(),
    }
}
