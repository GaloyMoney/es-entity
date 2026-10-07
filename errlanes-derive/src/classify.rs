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

use crate::std_error::{self, DisplayDefault, Unit};

#[derive(Default)]
struct ClassifyMeta {
    lane: Option<Lane>,
    delegate: bool,
    narrow: Option<Narrow>,
    from: bool,
    with: Option<Path>,
    lanes: Vec<Ident>,
    saw_pure_marker: bool,
    error_manual: bool,
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

fn parse_classify_meta(attrs: &[syn::Attribute], is_type: bool) -> syn::Result<ClassifyMeta> {
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
                "error" => {
                    if !is_type {
                        return Err(nested.error(
                            "`error = manual` is type-level only; put it on the enum/struct, \
                             not a variant",
                        ));
                    }
                    nested.input.parse::<Token![=]>()?;
                    let which: Ident = nested.input.parse()?;
                    if which != "manual" {
                        return Err(nested
                            .error("unknown `error` value, expected `error = manual`"));
                    }
                    meta.error_manual = true;
                }
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
                "code" | "level" | "code_prefix" | "code_and_level_from" | "origin" => {
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
        Lane::Denied => quote! {
            errlanes::Fail::Denied(errlanes::Denied::from_error(#value))
        },
    }
}

/// The field a `delegate`/`from` acts on, and how to spell it.
pub(crate) struct Payload<'a> {
    pub(crate) ty: &'a syn::Type,
    /// This field's index among its siblings — the same index
    /// [`crate::std_error::field_binding`] uses to name it.
    pub(crate) index: usize,
    /// Binds the payload to `p`: `(p)`, `{ name: p }` or `{ name: p, .. }`.
    pattern: TokenStream,
    /// Wraps `value`: `(value)` or `{ name: value }`. `None` when sibling
    /// fields exist — `delegate` can ignore them, `from` cannot invent them.
    constructor: Option<TokenStream>,
}

/// The only field, or — among several named fields — the one errlanes (like
/// `thiserror`) treats as the cause: marked `#[source]`, or named `source`.
/// Tuple fields cannot be singled out among siblings.
pub(crate) fn payload_field<'a>(
    fields: &'a Fields,
    span: &Ident,
    what: &str,
) -> syn::Result<Payload<'a>> {
    match fields {
        Fields::Unnamed(f) if f.unnamed.len() == 1 => Ok(Payload {
            ty: &f.unnamed[0].ty,
            index: 0,
            pattern: quote! { (p) },
            constructor: Some(quote! { (value) }),
        }),
        Fields::Named(f) if f.named.len() == 1 => {
            let name = f.named[0].ident.as_ref().expect("named field");
            Ok(Payload {
                ty: &f.named[0].ty,
                index: 0,
                pattern: quote! { { #name: p } },
                constructor: Some(quote! { { #name: value } }),
            })
        }
        Fields::Named(f) => {
            let is_cause = |field: &syn::Field| {
                field.attrs.iter().any(|a| a.path().is_ident("source"))
                    || field.ident.as_ref().is_some_and(|i| i == "source")
            };
            let mut causes = f
                .named
                .iter()
                .enumerate()
                .filter(|(_, field)| is_cause(field));
            match (causes.next(), causes.next()) {
                (Some((index, field)), None) => {
                    let name = field.ident.as_ref().expect("named field");
                    Ok(Payload {
                        ty: &field.ty,
                        index,
                        pattern: quote! { { #name: p, .. } },
                        constructor: None,
                    })
                }
                (None, _) => Err(syn::Error::new_spanned(
                    span,
                    format!(
                        "{what} with several fields needs the payload singled out: mark it \
                         `#[source]` or name it `source`"
                    ),
                )),
                (Some(_), Some(_)) => Err(syn::Error::new_spanned(
                    span,
                    format!("{what} found more than one `#[source]`/`source` field"),
                )),
            }
        }
        _ => Err(syn::Error::new_spanned(
            span,
            format!(
                "{what} needs a field to hold the payload, e.g. `Variant(Inner)` or \
                 `Variant {{ source: Inner }}`"
            ),
        )),
    }
}

pub(crate) fn from_impl(
    target: &Ident,
    generics: &syn::Generics,
    variant: Option<&Ident>,
    payload: &Payload<'_>,
    span: &Ident,
) -> syn::Result<TokenStream> {
    let Some(constructor) = &payload.constructor else {
        return Err(syn::Error::new_spanned(
            span,
            "`#[classify(from)]` needs the payload to be the only field: `From` has nothing to \
             fill its siblings with",
        ));
    };
    let field_ty = payload.ty;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let build = match variant {
        Some(v) => quote! { Self::#v #constructor },
        None => quote! { Self #constructor },
    };
    Ok(quote! {
        impl #impl_generics From<#field_ty> for #target #ty_generics #where_clause {
            fn from(value: #field_ty) -> Self {
                #build
            }
        }
    })
}

fn with_impl(
    ident: &Ident,
    generics: &syn::Generics,
    with: &Path,
    lanes: &[Ident],
    error_manual: bool,
    attrs: &[syn::Attribute],
    field_types: &[syn::Type],
) -> syn::Result<TokenStream> {
    if lanes.is_empty() {
        return Err(syn::Error::new_spanned(
            with,
            "`with = ..` requires `lanes(..)` naming at least one lane",
        ));
    }
    let classify_extra = std_error::classify_bounds(generics, field_types);
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let where_tokens = std_error::merge_where(where_clause, &classify_extra);
    let mut out = quote! {
        impl #impl_generics errlanes::Classify for #ident #ty_generics #where_tokens {
            type Rejected = core::convert::Infallible;
            type Lanes = errlanes::lanes!(#(#lanes),*);

            fn classify(self) -> errlanes::Fail<Self::Rejected, Self::Lanes> {
                (#with(self)).into()
            }
        }
    };
    if !error_manual {
        let fields = Fields::Unit;
        let error = std_error::take_error_lit(attrs)?;
        // `fields` is always `Fields::Unit` here, so `emit`'s `extra`
        // bounds are always empty — a type-level `#[error(..)]` literal
        // with no fields to bind can never reference a generic parameter.
        let (display_tokens, _extra) = std_error::emit(
            ident,
            generics,
            DisplayDefault::Name,
            &[Unit {
                pat: quote! { _ },
                fields: &fields,
                error,
                name: ident.clone(),
                source: None,
            }],
        )?;
        out.extend(display_tokens);
    }
    Ok(out)
}

fn derive_laned_struct(ast: &syn::DeriveInput, data: &syn::DataStruct) -> syn::Result<TokenStream> {
    let ident = &ast.ident;
    let meta = parse_classify_meta(&ast.attrs, true)?;
    let mut out = TokenStream::new();

    let from_payload = if meta.from {
        let payload = payload_field(&data.fields, ident, "`#[classify(from)]` on a struct")?;
        out.extend(from_impl(ident, &ast.generics, None, &payload, ident)?);
        Some(payload)
    } else {
        None
    };

    if let Some(with) = &meta.with {
        out.extend(with_impl(
            ident,
            &ast.generics,
            with,
            &meta.lanes,
            meta.error_manual,
            &ast.attrs,
            &std_error::field_types(&data.fields),
        )?);
        return Ok(out);
    }

    let mut display_extra: Vec<TokenStream> = Vec::new();
    if !meta.error_manual {
        std_error::reject_from_field_attr(&data.fields)?;
        let source = match &from_payload {
            Some(p) => Some({
                let name = std_error::field_binding(&data.fields, p.index);
                quote! { #name }
            }),
            None => std_error::marked_source(&data.fields, ident)?,
        };
        let pat = std_error::bind_pattern(quote! { Self }, &data.fields);
        let error = std_error::take_error_lit(&ast.attrs)?;
        let (display_tokens, extra) = std_error::emit(
            ident,
            &ast.generics,
            DisplayDefault::Name,
            &[Unit {
                pat,
                fields: &data.fields,
                error,
                name: ident.clone(),
                source,
            }],
        )?;
        out.extend(display_tokens);
        display_extra = extra;
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
    // `display_extra` (from `emit`, above) covers `Self: Display` for a
    // generic field a message actually formats; `classify_bounds` covers
    // the rest of `Classify`'s own supertrait structurally. Both are
    // needed for `Self: Error + Send + Sync + 'static` to hold.
    let mut classify_extra =
        std_error::classify_bounds(&ast.generics, &std_error::field_types(&data.fields));
    classify_extra.extend(display_extra);
    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();
    let where_tokens = std_error::merge_where(where_clause, &classify_extra);
    out.extend(quote! {
        impl #impl_generics errlanes::Classify for #ident #ty_generics #where_tokens {
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
    let type_meta = parse_classify_meta(&ast.attrs, true)?;

    if let Some(with) = &type_meta.with {
        let field_types: Vec<syn::Type> = data
            .variants
            .iter()
            .flat_map(|v| std_error::field_types(&v.fields))
            .collect();
        return with_impl(
            ident,
            &ast.generics,
            with,
            &type_meta.lanes,
            type_meta.error_manual,
            &ast.attrs,
            &field_types,
        );
    }

    // A type-level lane applies uniformly to every variant: the whole enum
    // value is the payload's source, so no per-variant attribute (or even a
    // per-variant match) is needed for *classification*. `#[error(..)]` is
    // different: a variant with its own fields still wants its own message
    // (see the `errlanes-classify-derive-codegen-gaps` handoff's Finding 1),
    // so each variant gets its own `Unit`, falling back to the type-level
    // `#[error(..)]` (if any) and then to the type's own snake_cased name —
    // exactly today's rendering — only when that variant has no `#[error]`
    // of its own.
    if let Some(lane) = &type_meta.lane {
        let lanes_ty = lane_profile_ty(&type_meta, None)?;
        let body = static_lane_expr(lane, quote! { self });
        let field_types: Vec<syn::Type> = data
            .variants
            .iter()
            .flat_map(|v| std_error::field_types(&v.fields))
            .collect();
        let mut classify_extra = std_error::classify_bounds(&ast.generics, &field_types);

        let display_tokens = if !type_meta.error_manual {
            let type_level_error = std_error::take_error_lit(&ast.attrs)?;
            let mut display_units = Vec::with_capacity(data.variants.len());
            for variant in &data.variants {
                std_error::reject_from_field_attr(&variant.fields)?;
                let variant_ident = &variant.ident;
                let source = std_error::marked_source(&variant.fields, variant_ident)?;
                let pat = std_error::bind_pattern(quote! { Self::#variant_ident }, &variant.fields);
                let error = match std_error::take_error_lit(&variant.attrs)? {
                    Some(spec) => Some(spec),
                    None => type_level_error.clone(),
                };
                display_units.push(Unit {
                    pat,
                    fields: &variant.fields,
                    error,
                    // The *type*'s name, not the variant's: with no
                    // `#[error(..)]` at all (type- or variant-level), this
                    // must keep rendering exactly what it renders today —
                    // the snake_cased type name, uniformly across variants.
                    name: ident.clone(),
                    source,
                });
            }
            let (tokens, extra) =
                std_error::emit(ident, &ast.generics, DisplayDefault::Name, &display_units)?;
            // See `derive_laned_struct`: `extra` covers `Self: Display` for
            // a generic field some variant's message actually formats.
            classify_extra.extend(extra);
            Some(tokens)
        } else {
            None
        };

        let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();
        let where_tokens = std_error::merge_where(where_clause, &classify_extra);
        let mut out = quote! {
            impl #impl_generics errlanes::Classify for #ident #ty_generics #where_tokens {
                type Rejected = core::convert::Infallible;
                type Lanes = #lanes_ty;

                fn classify(self) -> errlanes::Fail<Self::Rejected, Self::Lanes> {
                    #body
                }
            }
        };
        if let Some(tokens) = display_tokens {
            out.extend(tokens);
        }
        return Ok(out);
    }

    // Otherwise every variant declares its own lane or `delegate`.
    struct Resolved<'a> {
        variant: &'a syn::Variant,
        meta: ClassifyMeta,
        payload: Option<Payload<'a>>,
    }
    let mut resolved = Vec::new();
    for variant in &data.variants {
        let meta = parse_classify_meta(&variant.attrs, false)?;
        if !meta.is_laned() {
            return Err(syn::Error::new_spanned(
                &variant.ident,
                "every variant needs a lane (`fatal(Kind)`/`transient(Kind)`/`denied`) or \
                 `delegate`, or put one lane on the enum itself",
            ));
        }
        let payload = if meta.delegate || meta.from {
            Some(payload_field(
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
            payload,
        });
    }

    let mut lanes_ty = quote! { errlanes::profile::NoLanes };
    for r in &resolved {
        let contribution = lane_profile_ty(&r.meta, r.payload.as_ref().map(|p| p.ty))?;
        lanes_ty = quote! { <#lanes_ty as errlanes::profile::Union<#contribution>>::Out };
    }

    // Folded via `RejectedUnion`, not "the first delegate": at most one
    // delegate may carry a genuine rejection, but which one is first is an
    // accident of declaration order, and a never-rejecting source (a
    // `classify-sqlx`-blessed `sqlx::Error`, say) is just as likely to lead.
    // Folding order-independently means whichever delegate turns out to
    // reject, `Self::Rejected` resolves to its type — and two delegates
    // that both genuinely (and differently) reject become a compile error
    // from `RejectedUnion` itself, not a silent wrong answer.
    let mut rejected_ty = quote! { core::convert::Infallible };
    for r in &resolved {
        if r.meta.delegate && r.meta.narrow != Some(Narrow::Rejected) {
            let field_ty = r.payload.as_ref().unwrap().ty;
            let contribution = quote! { <#field_ty as errlanes::Classify>::Rejected };
            rejected_ty = quote! { <#rejected_ty as errlanes::RejectedUnion<#contribution>>::Out };
        }
    }

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
            let payload = r.payload.as_ref().unwrap();
            let field_ty = payload.ty;
            let pattern = &payload.pattern;
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
                _ => quote! { errlanes::Fail::widen(#narrowed) },
            };
            arms.push(quote! { Self::#variant_ident #pattern => #expr });
        }
        if r.meta.from {
            from_impls.extend(from_impl(
                ident,
                &ast.generics,
                Some(variant_ident),
                r.payload.as_ref().unwrap(),
                variant_ident,
            )?);
        }
    }

    let field_types: Vec<syn::Type> = resolved
        .iter()
        .flat_map(|r| std_error::field_types(&r.variant.fields))
        .collect();
    let mut classify_extra = std_error::classify_bounds(&ast.generics, &field_types);

    let display_tokens = if !type_meta.error_manual {
        let mut display_units = Vec::with_capacity(resolved.len());
        for r in &resolved {
            std_error::reject_from_field_attr(&r.variant.fields)?;
            let variant_ident = &r.variant.ident;
            let source = match &r.payload {
                Some(p) => {
                    let name = std_error::field_binding(&r.variant.fields, p.index);
                    Some(quote! { #name })
                }
                None => std_error::marked_source(&r.variant.fields, variant_ident)?,
            };
            let pat = std_error::bind_pattern(quote! { Self::#variant_ident }, &r.variant.fields);
            let error = std_error::take_error_lit(&r.variant.attrs)?;
            display_units.push(Unit {
                pat,
                fields: &r.variant.fields,
                error,
                name: variant_ident.clone(),
                source,
            });
        }
        let (tokens, extra) =
            std_error::emit(ident, &ast.generics, DisplayDefault::Name, &display_units)?;
        // See `derive_laned_struct`: `extra` covers `Self: Display` for a
        // generic field some variant's message actually formats.
        classify_extra.extend(extra);
        Some(tokens)
    } else {
        None
    };

    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();
    let where_tokens = std_error::merge_where(where_clause, &classify_extra);
    let mut out = quote! {
        impl #impl_generics errlanes::Classify for #ident #ty_generics #where_tokens {
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
    if let Some(tokens) = display_tokens {
        out.extend(tokens);
    }

    Ok(out)
}

pub fn derive(ast: &syn::DeriveInput) -> darling::Result<TokenStream> {
    let type_meta = parse_classify_meta(&ast.attrs, true).map_err(darling::Error::from)?;
    let any_variant_laned = match &ast.data {
        syn::Data::Enum(data) => data
            .variants
            .iter()
            .map(|v| parse_classify_meta(&v.attrs, false))
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
