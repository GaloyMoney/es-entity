//! `#[derive(errlanes::Classify)]`.
//!
//! **Pure mode**: no variant and no type-level `#[classify(..)]` item names a
//! lane (`fatal`/`transient`/`denied`), `delegate`, or `with` — the type
//! emits exactly what `#[derive(errlanes::Rejection)]` emits today (that
//! derive's own `impl<R: Rejection> Classify for R` blanket then supplies
//! `Classify`). Implemented by renaming every `#[classify(..)]` attribute to
//! `#[rejection(..)]` and handing the whole, unexamined `DeriveInput` to
//! [`crate::rejection::derive`] — identical grammar, same engine.
//!
//! **Laned mode**: otherwise. Emits `impl Classify` and nothing else.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Fields, Ident, Path, Token, punctuated::Punctuated};

#[derive(Default)]
struct ClassifyMeta {
    lane: Option<Lane>,
    delegate: bool,
    narrow: Option<Narrow>,
    from: bool,
    with: Option<Path>,
    lanes: Vec<Ident>,
    saw_pure_marker: bool,
}

enum Lane {
    Fatal(Ident),
    Transient(Ident),
    Denied,
}

#[derive(PartialEq, Eq)]
enum Narrow {
    Denied,
    Rejected,
}

impl ClassifyMeta {
    fn is_laned(&self) -> bool {
        self.lane.is_some() || self.delegate || self.with.is_some()
    }
}

fn parse_classify_meta(attrs: &[syn::Attribute]) -> syn::Result<ClassifyMeta> {
    let mut meta = ClassifyMeta::default();
    for attr in attrs {
        if !attr.path().is_ident("classify") {
            continue;
        }
        attr.parse_nested_meta(|nested| {
            let key = nested
                .path
                .get_ident()
                .map(|i| i.to_string())
                .unwrap_or_default();
            match key.as_str() {
                "fatal" => {
                    let content;
                    syn::parenthesized!(content in nested.input);
                    meta.lane = Some(Lane::Fatal(content.parse()?));
                }
                "transient" => {
                    let content;
                    syn::parenthesized!(content in nested.input);
                    meta.lane = Some(Lane::Transient(content.parse()?));
                }
                "denied" => meta.lane = Some(Lane::Denied),
                "delegate" => meta.delegate = true,
                "narrow" => {
                    let content;
                    syn::parenthesized!(content in nested.input);
                    let which: Ident = content.parse()?;
                    meta.narrow = Some(match which.to_string().as_str() {
                        "Denied" => Narrow::Denied,
                        "Rejected" => Narrow::Rejected,
                        "Transient" => {
                            return Err(nested.error(
                                "narrow(Transient) needs an attempt count; narrow it in the \
                                 retry loop with `Fail::narrow_transient`/`Fault::narrow_transient`, \
                                 not in `derive(Classify)`",
                            ));
                        }
                        other => {
                            return Err(nested.error(format!(
                                "unknown narrow target `{other}`, expected `Denied` or `Rejected`"
                            )));
                        }
                    });
                }
                "from" => meta.from = true,
                "with" => {
                    nested.input.parse::<Token![=]>()?;
                    meta.with = Some(nested.input.parse()?);
                }
                "lanes" => {
                    let content;
                    syn::parenthesized!(content in nested.input);
                    let list = Punctuated::<Ident, Token![,]>::parse_terminated(&content)?;
                    meta.lanes = list.into_iter().collect();
                }
                // Pure-mode items: meaningless here (laned mode never reaches
                // this type's attrs again — pure mode forwards the raw
                // tokens to `rejection::derive` instead), so just consume
                // them so `parse_nested_meta` does not choke on `= value` or
                // `(..)` it does not otherwise recognise.
                "code" | "level" | "code_prefix" | "forward" | "origin" => {
                    meta.saw_pure_marker = true;
                    if nested.input.peek(Token![=]) {
                        nested.input.parse::<Token![=]>()?;
                        let _: syn::Expr = nested.input.parse()?;
                    } else if nested.input.peek(syn::token::Paren) {
                        let content;
                        syn::parenthesized!(content in nested.input);
                        let _: proc_macro2::TokenStream = content.parse()?;
                    }
                }
                other => {
                    return Err(nested.error(format!("unknown `classify` item `{other}`")));
                }
            }
            Ok(())
        })?;
    }
    Ok(meta)
}

/// Renames every `#[classify(..)]` on the type (and, for an enum, every
/// variant) to `#[rejection(..)]`, leaving every other attribute — including
/// `#[lift(..)]`, `#[error(..)]`, `#[derive(..)]` — untouched, then hands the
/// result to [`crate::rejection::derive`]. Pure mode's grammar is, by
/// design, identical to `derive(Rejection)`'s; this is that identity made
/// literal instead of re-implemented.
fn derive_pure(ast: &syn::DeriveInput) -> darling::Result<TokenStream> {
    let mut ast = ast.clone();
    rename_classify_attrs(&mut ast.attrs);
    match &mut ast.data {
        syn::Data::Enum(data) => {
            for variant in &mut data.variants {
                rename_classify_attrs(&mut variant.attrs);
            }
        }
        syn::Data::Struct(data) => {
            for field in data.fields.iter_mut() {
                rename_classify_attrs(&mut field.attrs);
            }
        }
        syn::Data::Union(_) => {}
    }
    crate::rejection::derive(&ast)
}

fn rename_classify_attrs(attrs: &mut [syn::Attribute]) {
    for attr in attrs.iter_mut() {
        if let syn::Meta::List(list) = &mut attr.meta
            && list.path.is_ident("classify")
        {
            list.path = syn::parse_quote!(rejection);
        }
    }
}

/// The lane-profile *type* a single variant/struct contributes to the
/// enclosing `Lanes` union — `errlanes::lanes!(..)` for a static lane, or
/// `<FieldTy as errlanes::Classify>::Lanes` (optionally projected through
/// `WithoutDenied`) for a `delegate`.
fn lane_profile_ty(meta: &ClassifyMeta, field_ty: Option<&syn::Type>) -> syn::Result<TokenStream> {
    if let Some(lane) = &meta.lane {
        return Ok(match lane {
            Lane::Fatal(_) => quote! { errlanes::lanes!(Fatal) },
            Lane::Transient(_) => quote! { errlanes::lanes!(Transient) },
            Lane::Denied => quote! { errlanes::lanes!(Denied) },
        });
    }
    if meta.delegate {
        let field_ty = field_ty.expect("delegate requires a field");
        return Ok(match meta.narrow {
            Some(Narrow::Denied) => quote! {
                <<#field_ty as errlanes::Classify>::Lanes as errlanes::LaneProfile>::WithoutDenied
            },
            _ => quote! { <#field_ty as errlanes::Classify>::Lanes },
        });
    }
    unreachable!("lane_profile_ty called on a variant with neither a lane nor delegate")
}

/// `self`/`v` (already bound to the whole value, so a handler or test can
/// `downcast_ref` the wrapper itself out of the chain) constructed as the
/// static lane's payload.
fn static_lane_expr(lane: &Lane, value: TokenStream) -> TokenStream {
    match lane {
        Lane::Fatal(kind) => quote! {
            errlanes::Fail::Fatal(errlanes::Fatal::from_error(errlanes::FatalKind::#kind, #value))
        },
        Lane::Transient(kind) => quote! {
            errlanes::Fail::Transient(errlanes::Transient::from_error(errlanes::TransientKind::#kind, #value))
        },
        // `Denied` carries no source today (see `errlanes::Denied`'s docs) —
        // the value is still consumed by the match arm, just not kept.
        Lane::Denied => quote! { errlanes::Fail::Denied(errlanes::Denied::default()) },
    }
}

fn single_field<'a>(fields: &'a Fields, span: &Ident, what: &str) -> syn::Result<&'a syn::Type> {
    match fields {
        Fields::Unnamed(f) if f.unnamed.len() == 1 => Ok(&f.unnamed[0].ty),
        Fields::Named(f) if f.named.len() == 1 => Ok(&f.named[0].ty),
        _ => Err(syn::Error::new_spanned(
            span,
            format!("{what} requires exactly one field"),
        )),
    }
}

fn from_impl(
    target: &Ident,
    generics: &syn::Generics,
    variant: Option<&Ident>,
    field_ty: &syn::Type,
) -> TokenStream {
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let build = match variant {
        Some(v) => quote! { Self::#v(value) },
        None => quote! { Self(value) },
    };
    quote! {
        impl #impl_generics From<#field_ty> for #target #ty_generics #where_clause {
            fn from(value: #field_ty) -> Self {
                #build
            }
        }
    }
}

fn with_impl(
    ident: &Ident,
    generics: &syn::Generics,
    with: &Path,
    lanes: &[Ident],
) -> syn::Result<TokenStream> {
    if lanes.is_empty() {
        return Err(syn::Error::new_spanned(
            with,
            "`with = ..` requires `lanes(..)` naming at least one lane",
        ));
    }
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics errlanes::Classify for #ident #ty_generics #where_clause {
            type Rejected = core::convert::Infallible;
            type Lanes = errlanes::lanes!(#(#lanes),*);

            fn classify(self) -> errlanes::Fail<Self::Rejected, Self::Lanes> {
                (#with(self)).into()
            }
        }
    })
}

fn derive_laned_struct(ast: &syn::DeriveInput, data: &syn::DataStruct) -> syn::Result<TokenStream> {
    let ident = &ast.ident;
    let meta = parse_classify_meta(&ast.attrs)?;
    let mut out = TokenStream::new();

    if meta.from {
        let field_ty = single_field(&data.fields, ident, "`#[classify(from)]` on a struct")?;
        out.extend(from_impl(ident, &ast.generics, None, field_ty));
    }

    if let Some(with) = &meta.with {
        out.extend(with_impl(ident, &ast.generics, with, &meta.lanes)?);
        return Ok(out);
    }

    let Some(lane) = &meta.lane else {
        return Err(syn::Error::new_spanned(
            ident,
            "a laned struct needs a lane (`#[classify(fatal(Kind))]`, `transient(Kind)`, or \
             `denied`) or `with = ..`; a struct naming neither a lane, `delegate`, nor `with` is \
             pure — derive `errlanes::Rejection` instead",
        ));
    };
    let lanes_ty = lane_profile_ty(&meta, None)?;
    let body = static_lane_expr(lane, quote! { self });
    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();
    out.extend(quote! {
        impl #impl_generics errlanes::Classify for #ident #ty_generics #where_clause {
            type Rejected = core::convert::Infallible;
            type Lanes = #lanes_ty;

            fn classify(self) -> errlanes::Fail<Self::Rejected, Self::Lanes> {
                #body
            }
        }
    });
    Ok(out)
}

fn derive_laned_enum(ast: &syn::DeriveInput, data: &syn::DataEnum) -> syn::Result<TokenStream> {
    let ident = &ast.ident;
    let type_meta = parse_classify_meta(&ast.attrs)?;

    if let Some(with) = &type_meta.with {
        return with_impl(ident, &ast.generics, with, &type_meta.lanes);
    }

    // A type-level lane applies uniformly to every variant: the whole enum
    // value is the payload's source, so no per-variant attribute (or even a
    // per-variant match) is needed at all.
    if let Some(lane) = &type_meta.lane {
        let lanes_ty = lane_profile_ty(&type_meta, None)?;
        let body = static_lane_expr(lane, quote! { self });
        let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();
        return Ok(quote! {
            impl #impl_generics errlanes::Classify for #ident #ty_generics #where_clause {
                type Rejected = core::convert::Infallible;
                type Lanes = #lanes_ty;

                fn classify(self) -> errlanes::Fail<Self::Rejected, Self::Lanes> {
                    #body
                }
            }
        });
    }

    // Otherwise every variant declares its own lane or `delegate`.
    struct Resolved<'a> {
        variant: &'a syn::Variant,
        meta: ClassifyMeta,
        field_ty: Option<&'a syn::Type>,
    }
    let mut resolved = Vec::new();
    for variant in &data.variants {
        let meta = parse_classify_meta(&variant.attrs)?;
        if !meta.is_laned() {
            return Err(syn::Error::new_spanned(
                &variant.ident,
                "every variant needs a lane (`fatal(Kind)`/`transient(Kind)`/`denied`) or \
                 `delegate`, or put one lane on the enum itself",
            ));
        }
        let field_ty = if meta.delegate || meta.from {
            Some(single_field(
                &variant.fields,
                &variant.ident,
                if meta.delegate {
                    "`delegate`"
                } else {
                    "`#[classify(from)]` on a variant"
                },
            )?)
        } else {
            None
        };
        if meta.narrow.is_some() && !meta.delegate {
            return Err(syn::Error::new_spanned(
                &variant.ident,
                "`narrow(..)` only applies to a `delegate` variant",
            ));
        }
        resolved.push(Resolved {
            variant,
            meta,
            field_ty,
        });
    }

    let mut lanes_ty = quote! { errlanes::profile::NoLanes };
    for r in &resolved {
        let contribution = lane_profile_ty(&r.meta, r.field_ty)?;
        lanes_ty = quote! { <#lanes_ty as errlanes::profile::Union<#contribution>>::Out };
    }

    let rejected_source = resolved.iter().find_map(|r| {
        (r.meta.delegate && r.meta.narrow != Some(Narrow::Rejected)).then(|| r.field_ty.unwrap())
    });
    let rejected_ty = match rejected_source {
        Some(ty) => quote! { <#ty as errlanes::Classify>::Rejected },
        None => quote! { core::convert::Infallible },
    };

    let mut arms = Vec::new();
    let mut from_impls = TokenStream::new();
    for r in &resolved {
        let variant_ident = &r.variant.ident;
        if let Some(lane) = &r.meta.lane {
            let pat = match &r.variant.fields {
                Fields::Unit => quote! { Self::#variant_ident },
                Fields::Unnamed(_) => quote! { Self::#variant_ident(..) },
                Fields::Named(_) => quote! { Self::#variant_ident { .. } },
            };
            let value = quote! { v };
            let expr = static_lane_expr(lane, value);
            arms.push(quote! { v @ #pat => #expr });
        } else if r.meta.delegate {
            let field_ty = r.field_ty.unwrap();
            let classify_call = quote! {
                <#field_ty as errlanes::Classify>::classify(p)
            };
            let narrowed = match r.meta.narrow {
                Some(Narrow::Denied) => quote! { #classify_call.narrow_denied() },
                Some(Narrow::Rejected) => quote! { #classify_call.narrow_rejected() },
                None => classify_call,
            };
            let expr = match r.meta.narrow {
                Some(Narrow::Rejected) => quote! { errlanes::Fail::from(#narrowed) },
                _ => quote! { errlanes::Fail::lift(#narrowed) },
            };
            arms.push(quote! { Self::#variant_ident(p) => #expr });
        }
        if r.meta.from {
            from_impls.extend(from_impl(
                ident,
                &ast.generics,
                Some(variant_ident),
                r.field_ty.unwrap(),
            ));
        }
    }

    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();
    let mut out = quote! {
        impl #impl_generics errlanes::Classify for #ident #ty_generics #where_clause {
            type Rejected = #rejected_ty;
            type Lanes = #lanes_ty;

            fn classify(self) -> errlanes::Fail<Self::Rejected, Self::Lanes> {
                match self {
                    #(#arms,)*
                }
            }
        }
    };
    out.extend(from_impls);
    Ok(out)
}

pub fn derive(ast: &syn::DeriveInput) -> darling::Result<TokenStream> {
    let type_meta = parse_classify_meta(&ast.attrs).map_err(darling::Error::from)?;
    let any_variant_laned = match &ast.data {
        syn::Data::Enum(data) => data
            .variants
            .iter()
            .map(|v| parse_classify_meta(&v.attrs))
            .collect::<syn::Result<Vec<_>>>()
            .map_err(darling::Error::from)?
            .iter()
            .any(ClassifyMeta::is_laned),
        _ => false,
    };

    if !type_meta.is_laned() && !any_variant_laned {
        return derive_pure(ast);
    }

    match &ast.data {
        syn::Data::Struct(data) => derive_laned_struct(ast, data).map_err(darling::Error::from),
        syn::Data::Enum(data) => derive_laned_enum(ast, data).map_err(darling::Error::from),
        syn::Data::Union(_) => Err(darling::Error::custom(
            "Classify cannot be derived for a union",
        )
        .with_span(&ast.ident)),
    }
}
