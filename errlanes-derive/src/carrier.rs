//! `#[derive(errlanes::Carrier)]` on a hand-written lane enum.
//!
//! The variant names are the profile: `Rejected(R)` makes the carrier
//! `Fail`-like, otherwise it is `Fault`-like, and `Denied` / `Transient` /
//! `Fatal` each switch their lane on.

use proc_macro2::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use syn::{
    DeriveInput, Fields, Generics, Ident, Path, Token, Type, WherePredicate,
    punctuated::Punctuated, spanned::Spanned,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum LaneKind {
    Rejected,
    Denied,
    Transient,
    Fatal,
}

impl LaneKind {
    fn from_ident(ident: &Ident) -> Option<Self> {
        match ident.to_string().as_str() {
            "Rejected" => Some(Self::Rejected),
            "Denied" => Some(Self::Denied),
            "Transient" => Some(Self::Transient),
            "Fatal" => Some(Self::Fatal),
            _ => None,
        }
    }
}

struct Lane<'a> {
    kind: LaneKind,
    ident: &'a Ident,
    ty: &'a Type,
}

fn parse_from(input: &DeriveInput) -> syn::Result<Vec<Path>> {
    let mut from = Vec::new();
    for attr in &input.attrs {
        if !attr.path().is_ident("carrier") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("from") {
                let content;
                syn::parenthesized!(content in meta.input);
                let paths = Punctuated::<Path, Token![,]>::parse_terminated(&content)?;
                from.extend(paths);
                Ok(())
            } else {
                Err(meta.error("unknown `carrier` option: expected `from(Path, ..)`"))
            }
        })?;
    }
    Ok(from)
}

fn collect_lanes(input: &DeriveInput) -> syn::Result<Vec<Lane<'_>>> {
    let syn::Data::Enum(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "`#[derive(errlanes::Carrier)]` needs an enum whose variants are lanes: \
             `Rejected(R)`, `Denied(..)`, `Transient(..)`, `Fatal(..)`",
        ));
    };
    if let Some(attr) = input
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
    if data.variants.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "a carrier needs at least one lane variant: `Rejected(R)`, `Denied(..)`, \
             `Transient(..)` or `Fatal(..)`",
        ));
    }
    let mut lanes: Vec<Lane<'_>> = Vec::new();
    for variant in &data.variants {
        let Some(kind) = LaneKind::from_ident(&variant.ident) else {
            return Err(syn::Error::new_spanned(
                &variant.ident,
                format!(
                    "`{}` is not a lane: carrier variants are named `Rejected`, `Denied`, \
                     `Transient` or `Fatal`, and the variant names are the profile",
                    variant.ident
                ),
            ));
        };
        if lanes.iter().any(|l| l.kind == kind) {
            return Err(syn::Error::new_spanned(
                &variant.ident,
                format!("duplicate lane variant `{}`", variant.ident),
            ));
        }
        if let Some((_, discriminant)) = &variant.discriminant {
            return Err(syn::Error::new_spanned(
                discriminant,
                "a carrier variant cannot have a discriminant",
            ));
        }
        let ty = match &variant.fields {
            Fields::Unnamed(f) if f.unnamed.len() == 1 => &f.unnamed[0].ty,
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    format!(
                        "a carrier variant is a one-field tuple variant: `{}(Payload)`",
                        variant.ident
                    ),
                ));
            }
        };
        lanes.push(Lane {
            kind,
            ident: &variant.ident,
            ty,
        });
    }
    Ok(lanes)
}

fn extended_where(generics: &Generics, extra: &[WherePredicate]) -> TokenStream {
    let existing = generics
        .where_clause
        .as_ref()
        .map(|w| w.predicates.iter().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let all = existing.iter().chain(extra.iter());
    quote! { where #(#all,)* }
}

pub fn derive(input: &DeriveInput) -> syn::Result<TokenStream> {
    let from = parse_from(input)?;
    let lanes = collect_lanes(input)?;
    let ident = &input.ident;
    let generics = &input.generics;
    let (_, ty_generics, _) = generics.split_for_impl();

    let has = |k: LaneKind| lanes.iter().any(|l| l.kind == k);
    let rejection: Option<&Type> = lanes
        .iter()
        .find(|l| l.kind == LaneKind::Rejected)
        .map(|l| l.ty);
    let (d, t, f) = (
        has(LaneKind::Denied),
        has(LaneKind::Transient),
        has(LaneKind::Fatal),
    );

    let profile = quote! { errlanes::profile::Profile<#d, #t, #f> };
    let rejected_ty = match rejection {
        Some(r) => quote! { #r },
        None => quote! { ::core::convert::Infallible },
    };
    let repr = match rejection {
        Some(r) => quote! { errlanes::Fail<#r, #profile> },
        None => quote! { errlanes::Fault<#profile> },
    };
    let repr_enum = match rejection {
        Some(_) => quote! { errlanes::Fail },
        None => quote! { errlanes::Fault },
    };

    // Every impl is bounded by the carrier's own coherence story: a rejection
    // type must be a `Rejection` (which brings `Error + Display + Send + Sync +
    // 'static`).
    let mut preds: Vec<WherePredicate> = Vec::new();
    if let Some(r) = rejection {
        preds.push(syn::parse_quote!(#r: errlanes::Rejection));
    }
    let where_base = extended_where(generics, &preds);
    let (impl_generics, _, _) = generics.split_for_impl();

    // `Display` -----------------------------------------------------------
    let display_arms = lanes.iter().map(|l| {
        let v = l.ident;
        let span = l.ty.span();
        if l.kind == LaneKind::Rejected {
            quote_spanned! {span=> Self::#v(x) => ::core::write!(f, "rejected: {}", x) }
        } else {
            quote_spanned! {span=> Self::#v(x) => ::core::fmt::Display::fmt(x, f) }
        }
    });
    let source_arms = lanes.iter().map(|l| {
        let v = l.ident;
        let span = l.ty.span();
        quote_spanned! {span=>
            Self::#v(x) => ::core::option::Option::Some(x as &(dyn ::std::error::Error + 'static))
        }
    });

    // `IntoLanes` ---------------------------------------------------------
    let into_lanes_arms = lanes.iter().map(|l| {
        let v = l.ident;
        let span = l.ty.span();
        quote_spanned! {span=> Self::#v(x) => errlanes::Fail::#v(x) }
    });

    // `Carrier` -----------------------------------------------------------
    let from_repr_arms = lanes.iter().map(|l| {
        let v = l.ident;
        let span = l.ty.span();
        quote_spanned! {span=> #repr_enum::#v(x) => Self::#v(x) }
    });
    let into_repr_arms = lanes.iter().map(|l| {
        let v = l.ident;
        let span = l.ty.span();
        quote_spanned! {span=> Self::#v(x) => #repr_enum::#v(x) }
    });
    let lane_ref_arms = lanes.iter().map(|l| {
        let v = l.ident;
        let span = l.ty.span();
        quote_spanned! {span=> Self::#v(x) => errlanes::LaneRef::#v(x) }
    });

    // Inherent methods ----------------------------------------------------
    let mut methods = TokenStream::new();
    for l in &lanes {
        let v = l.ident;
        let name = v.to_string().to_lowercase();
        let is = format_ident!("is_{}", name);
        let as_ = format_ident!("as_{}", name);
        let is_doc = format!(
            "Whether this is the `{v}` lane. Delegates to the `Fault` / `Fail` method of the \
             same name."
        );
        let as_doc = format!(
            "The `{v}` payload, if this is the `{v}` lane. Delegates to the `Fault` / `Fail` \
             method of the same name."
        );
        let payload_ty = match l.kind {
            LaneKind::Rejected => {
                let r = l.ty;
                quote! { #r }
            }
            LaneKind::Denied => quote! { errlanes::Denied },
            LaneKind::Transient => quote! { errlanes::Transient },
            LaneKind::Fatal => quote! { errlanes::Fatal },
        };
        methods.extend(quote! {
            #[doc = #is_doc]
            pub fn #is(&self) -> bool {
                ::core::matches!(self, Self::#v(_))
            }
            #[doc = #as_doc]
            pub fn #as_(&self) -> ::core::option::Option<&#payload_ty> {
                match self {
                    Self::#v(x) => ::core::option::Option::Some(x),
                    _ => ::core::option::Option::None,
                }
            }
        });
    }
    if t {
        methods.extend(quote! {
            /// See `Transient::is_congestion`. Delegates to the `Fault` / `Fail` method of the
            /// same name.
            pub fn is_congestion(&self) -> bool {
                self.as_transient().is_some_and(errlanes::Transient::is_congestion)
            }
            /// See `TransientKind::is_contention`. Delegates to the `Fault` / `Fail` method of
            /// the same name.
            pub fn is_contention(&self) -> bool {
                self.as_transient().is_some_and(errlanes::Transient::is_contention)
            }
        });
    }

    let from_impls = from.iter().map(|up| {
        quote! {
            impl #impl_generics ::core::convert::From<#up> for #ident #ty_generics #where_base {
                fn from(w: #up) -> Self {
                    <Self as errlanes::Carrier>::from_repr(
                        <#repr as errlanes::Absorb<#up>>::absorb(w)
                    )
                }
            }
        }
    });

    // The one blanket inbound `From`: generics plus `__W`.
    let mut blanket_generics = generics.clone();
    blanket_generics.params.push(syn::parse_quote!(
        __W: errlanes::IntoLanes<Kind = errlanes::kind::Plain>
    ));
    let (blanket_impl_generics, _, _) = blanket_generics.split_for_impl();
    let mut blanket_preds = preds.clone();
    blanket_preds.push(syn::parse_quote!(#repr: errlanes::Absorb<__W>));
    let where_blanket = extended_where(generics, &blanket_preds);

    Ok(quote! {
        impl #impl_generics ::core::fmt::Display for #ident #ty_generics #where_base {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                match self { #(#display_arms,)* }
            }
        }

        impl #impl_generics ::std::error::Error for #ident #ty_generics #where_base {
            // The lane payload itself, never `self`'s built-in: `Lane::of` and
            // `Fault::classify` downcast the payload directly.
            fn source(&self) -> ::core::option::Option<&(dyn ::std::error::Error + 'static)> {
                match self { #(#source_arms,)* }
            }
        }

        impl #impl_generics errlanes::IntoLanes for #ident #ty_generics #where_base {
            type Kind = errlanes::kind::Carrier;
            type Shape = errlanes::kind::CarrierShape;
            type Rejected = #rejected_ty;
            type Lanes = #profile;

            fn into_lanes(self) -> errlanes::Fail<#rejected_ty, #profile> {
                match self { #(#into_lanes_arms,)* }
            }
        }

        impl #impl_generics errlanes::Carrier for #ident #ty_generics #where_base {
            type Repr = #repr;

            fn from_repr(r: #repr) -> Self {
                match r { #(#from_repr_arms,)* }
            }

            fn into_repr(self) -> #repr {
                match self { #(#into_repr_arms,)* }
            }

            fn lanes(&self) -> errlanes::LaneRef<'_, #rejected_ty> {
                match self { #(#lane_ref_arms,)* }
            }
        }

        impl #blanket_impl_generics ::core::convert::From<__W> for #ident #ty_generics
            #where_blanket
        {
            fn from(w: __W) -> Self {
                <Self as errlanes::Carrier>::from_repr(
                    <#repr as errlanes::Absorb<__W>>::absorb(w)
                )
            }
        }

        #(#from_impls)*

        #[allow(unreachable_patterns)]
        impl #impl_generics #ident #ty_generics #where_base {
            /// Which lane this is. Delegates to the `Fault` / `Fail` method of the same name.
            pub fn lane(&self) -> errlanes::Lane {
                errlanes::Carrier::lanes(self).lane()
            }

            /// The operator-safe one-line text for this failure, exactly what `record` writes
            /// to `exception.message`. Delegates to the `Fault` / `Fail` method of the same
            /// name.
            pub fn message(&self) -> ::std::string::String {
                errlanes::Carrier::lanes(self).message()
            }

            #methods
        }
    })
}
