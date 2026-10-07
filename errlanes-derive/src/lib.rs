//! Proc macros for `errlanes`: `#[derive(Rejection)]`, `#[derive(Lift)]`, `#[derive(Carrier)]`,
//! `#[compose]`, and `#[instrument]`.

mod carrier;
mod classify;
mod composition;
mod instrument;
mod lift;
mod rejection;
mod std_error;

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

#[proc_macro_derive(Rejection, attributes(rejection, lift, error, source, from))]
pub fn derive_rejection(input: TokenStream) -> TokenStream {
    expand(input, rejection::derive)
}

#[proc_macro_derive(Classify, attributes(classify, rejection, lift, error, source, from))]
pub fn derive_classify(input: TokenStream) -> TokenStream {
    expand(input, classify::derive)
}

/// Derive error conversion mappings. A single-field tuple struct can project
/// a source struct field with `#[lift(Source, field = name)]`, generating
/// `From<Source>` independently of rejection metadata.
#[proc_macro_derive(Lift, attributes(lift))]
pub fn derive_lift(input: TokenStream) -> TokenStream {
    expand(input, |ast| lift::derive(ast).map_err(darling::Error::from))
}

/// Derive a [carrier](https://docs.rs/errlanes/latest/errlanes/trait.Carrier.html): a
/// crate-local lane enum that stands in for `Fault<lanes!(..)>` or `Fail<R, lanes!(..)>`.
///
/// ```ignore
/// #[derive(Debug, errlanes::Carrier)]
/// #[carrier(from(effects::HostFault))]
/// pub enum PartyFault {
///     /// the subject may not do this
///     Denied(errlanes::Denied),
///     Transient(errlanes::Transient),
///     Fatal(errlanes::Fatal),
/// }
///
/// #[derive(Debug, errlanes::Carrier)]
/// pub enum WriteError<R> {
///     Rejected(R),
///     Transient(errlanes::Transient),
///     Fatal(errlanes::Fatal),
/// }
/// ```
///
/// The variant names are the profile: each is a one-field tuple variant named from
/// `Rejected` / `Denied` / `Transient` / `Fatal`, at most one of each. `Rejected(T)` makes the
/// carrier `Fail`-like with rejection `T` (a concrete type or one of the enum's own generic
/// parameters); without it the carrier is `Fault`-like and needs at least one lane.
/// `#[carrier(from(Up, ..))]` lists other carriers that convert into this one by `?`.
/// `Debug` is yours to derive (`Error` needs it).
#[proc_macro_derive(Carrier, attributes(carrier))]
pub fn derive_carrier(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    match carrier::derive(&ast) {
        Ok(tokens) => resolve_runtime(tokens).into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// Compose rejection families from a list of sources: `Source` imports every
/// case under its original variant name, `Source as Prefix` under a prefixed
/// one. Collisions between unprefixed sources require a local
/// `#[compose(merge)]` variant; use `#[compose(merge(SourceA::Case,
/// SourceB::OtherCase))]` to select participants explicitly. Merged variants
/// own their metadata through an explicit rejection code or payload
/// delegation. Every source gets a total lift.
#[proc_macro_attribute]
pub fn compose(args: TokenStream, input: TokenStream) -> TokenStream {
    match syn::parse::<syn::ItemEnum>(input)
        .and_then(|item| composition::compose(args.into(), item))
    {
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

#[proc_macro_attribute]
pub fn instrument(args: TokenStream, input: TokenStream) -> TokenStream {
    match instrument::expand(args.into(), input.into()) {
        Ok(t) => resolve_runtime(t).into(),
        Err(e) => e.to_compile_error().into(),
    }
}
