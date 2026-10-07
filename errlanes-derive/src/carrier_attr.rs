//! `#[errlanes::fault(..)]` / `#[errlanes::fail(..)]`: replace a unit struct
//! with a lane enum carrying exactly the declared lanes, then hand it to the
//! `Carrier` derive. There is one codegen path.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    Ident, ItemStruct, Path, Token, Type,
    parse::{ParseStream, Parser},
    punctuated::Punctuated,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    Fault,
    Fail,
}

impl Flavor {
    fn name(self) -> &'static str {
        match self {
            Flavor::Fault => "fault",
            Flavor::Fail => "fail",
        }
    }
}

struct Args {
    rejection: Option<Type>,
    lanes: Vec<Ident>,
    from: Vec<Path>,
}

fn parse_args(flavor: Flavor, input: ParseStream) -> syn::Result<Args> {
    let rejection = match flavor {
        Flavor::Fail => {
            let ty: Type = input.parse()?;
            input.parse::<Token![;]>().map_err(|e| {
                syn::Error::new(
                    e.span(),
                    "expected `;` after the rejection type: \
                     `#[errlanes::fail(Rejection; Transient, Fatal)]`",
                )
            })?;
            Some(ty)
        }
        Flavor::Fault => None,
    };

    let mut lanes: Vec<Ident> = Vec::new();
    while !input.is_empty() && !input.peek(Token![;]) {
        let lane: Ident = input.parse()?;
        if lane == "Rejected" {
            return Err(syn::Error::new_spanned(
                &lane,
                "`Rejected` is not a fault lane: it is selected by the carrier, not the \
                 profile. Use `#[errlanes::fail(YourRejection; ..)]` instead of listing \
                 `Rejected`.",
            ));
        }
        if !["Denied", "Transient", "Fatal"]
            .iter()
            .any(|known| lane == *known)
        {
            return Err(syn::Error::new_spanned(
                &lane,
                format!("unknown lane `{lane}`: expected `Denied`, `Transient`, or `Fatal`"),
            ));
        }
        if lanes.contains(&lane) {
            return Err(syn::Error::new_spanned(
                &lane,
                format!("duplicate lane `{lane}`"),
            ));
        }
        lanes.push(lane);
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        } else {
            break;
        }
    }

    let mut from = Vec::new();
    if input.peek(Token![;]) {
        input.parse::<Token![;]>()?;
        if !input.is_empty() {
            let kw: Ident = input.parse()?;
            if kw != "from" {
                return Err(syn::Error::new_spanned(
                    &kw,
                    "expected `from(Carrier, ..)` after the lane list",
                ));
            }
            let content;
            syn::parenthesized!(content in input);
            from = Punctuated::<Path, Token![,]>::parse_terminated(&content)?
                .into_iter()
                .collect();
            if !input.is_empty() {
                return Err(input.error("unexpected tokens after `from(..)`"));
            }
        }
    } else if !input.is_empty() {
        return Err(input.error("expected `,`, `;` or the end of the lane list"));
    }

    if flavor == Flavor::Fault && lanes.is_empty() {
        return Err(input.error("a fault carrier needs at least one lane"));
    }

    Ok(Args {
        rejection,
        lanes,
        from,
    })
}

pub fn expand(flavor: Flavor, args: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let mut parsed = Parser::parse2(|input: ParseStream| parse_args(flavor, input), args)?;
    let name = flavor.name();

    let item: ItemStruct = syn::parse2(item).map_err(|e| {
        syn::Error::new(
            e.span(),
            format!(
                "`#[errlanes::{name}]` replaces a unit struct with a lane enum; \
                 write `pub struct Name;`"
            ),
        )
    })?;
    if !matches!(item.fields, syn::Fields::Unit) {
        return Err(syn::Error::new_spanned(
            &item.ident,
            format!(
                "`#[errlanes::{name}]` replaces a unit struct with a lane enum; \
                 write `pub struct Name;`"
            ),
        ));
    }
    if let Some(attr) = item
        .attrs
        .iter()
        .find(|a| a.path().is_ident("non_exhaustive"))
    {
        return Err(syn::Error::new_spanned(
            attr,
            "`#[non_exhaustive]` is not allowed on a carrier: its variant set is the profile, \
             and exhaustive matching is the point",
        ));
    }

    // Variant order is always `Rejected, Denied, Transient, Fatal`.
    let mut variants = Vec::new();
    if let Some(r) = parsed.rejection.take() {
        variants.push(quote! { Rejected(#r) });
    }
    for lane in ["Denied", "Transient", "Fatal"] {
        if let Some(l) = parsed.lanes.iter().find(|l| **l == lane) {
            variants.push(quote! { #l(errlanes::#l) });
        }
    }

    let ItemStruct {
        attrs,
        vis,
        ident,
        generics,
        ..
    } = item;
    let where_clause = &generics.where_clause;
    let from_attr = if parsed.from.is_empty() {
        quote! {}
    } else {
        let from = &parsed.from;
        quote! { #[carrier(from(#(#from),*))] }
    };

    Ok(quote! {
        #(#attrs)*
        #[derive(::core::fmt::Debug, errlanes::Carrier)]
        #from_attr
        #vis enum #ident #generics #where_clause {
            #(#variants,)*
        }
    })
}
