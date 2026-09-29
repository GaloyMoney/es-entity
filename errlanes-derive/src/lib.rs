//! Proc macros for `errlanes`: `#[derive(Rejection)]`, `#[derive(Failure)]`,
//! `#[derive(Classify)]`.

mod classify;
mod failure;
mod rejection;

use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

fn expand(
    input: TokenStream,
    f: impl FnOnce(&DeriveInput) -> darling::Result<proc_macro2::TokenStream>,
) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    match f(&ast) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.write_errors().into(),
    }
}

#[proc_macro_derive(Rejection, attributes(rejection))]
pub fn derive_rejection(input: TokenStream) -> TokenStream {
    expand(input, rejection::derive)
}

#[proc_macro_derive(Failure, attributes(failure))]
pub fn derive_failure(input: TokenStream) -> TokenStream {
    expand(input, failure::derive)
}

#[proc_macro_derive(Classify, attributes(lane))]
pub fn derive_classify(input: TokenStream) -> TokenStream {
    expand(input, classify::derive)
}
