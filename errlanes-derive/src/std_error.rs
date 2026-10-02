//! Shared `std::fmt::Display` + `std::error::Error` emission for
//! `#[derive(errlanes::Rejection)]` and `#[derive(errlanes::Classify)]`. See
//! the `errlanes-native-display-error` handoff for the design this
//! implements: a rejection displays its code, a laned wrapper displays its
//! snake_case name, and both read `source()` off the same field marker rule
//! (`#[source]` / a field named `source` / a `delegate` or `from` payload).

use convert_case::{Case, Casing};
use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use syn::{Fields, Ident, LitStr, Type};

/// What `Display` falls back to when a unit carries no explicit
/// `#[error("..")]`.
pub(crate) enum DisplayDefault {
    /// `<Self as errlanes::Rejection>::code(self)` — identical text for
    /// every unit, computed once by the already-generated `code()` method.
    Code,
    /// This unit's own name, lowercased to `snake_case`.
    Name,
}

/// One struct-as-a-whole, or one enum variant.
pub(crate) struct Unit<'a> {
    /// The match pattern, destructuring every field via [`bind_pattern`].
    pub pat: TokenStream,
    pub fields: &'a Fields,
    /// Explicit `#[error("..")]`, already pulled off the AST by the caller.
    pub error: Option<LitStr>,
    /// This unit's own name, used by the `Name` default.
    pub name: Ident,
    /// The locally-bound field name that is `source()`, if any.
    pub source: Option<TokenStream>,
}

fn field_list(fields: &Fields) -> Vec<&syn::Field> {
    match fields {
        Fields::Named(f) => f.named.iter().collect(),
        Fields::Unnamed(f) => f.unnamed.iter().collect(),
        Fields::Unit => Vec::new(),
    }
}

/// The local identifier a field is bound to in a generated match pattern:
/// its own name for a named field, `field_{index}` for a tuple field.
pub(crate) fn field_binding(fields: &Fields, index: usize) -> Ident {
    match field_list(fields)[index].ident.clone() {
        Some(name) => name,
        None => format_ident!("field_{index}"),
    }
}

fn bound_fields(fields: &Fields) -> Vec<Ident> {
    (0..field_list(fields).len())
        .map(|i| field_binding(fields, i))
        .collect()
}

/// `path` destructuring every field by [`field_binding`]: `path`,
/// `path(field_0, field_1)`, or `path { a, b }`.
pub(crate) fn bind_pattern(path: TokenStream, fields: &Fields) -> TokenStream {
    let names = bound_fields(fields);
    match fields {
        Fields::Named(_) => quote! { #path { #(#names),* } },
        Fields::Unnamed(_) => quote! { #path(#(#names),*) },
        Fields::Unit => path,
    }
}

/// The field marked `#[source]`, or named `source` — the shape errlanes
/// (like `thiserror`) treats as the cause when nothing else says otherwise.
/// `None` if nothing is marked: a bare field is never assumed to be the
/// source. An error if more than one field is marked.
pub(crate) fn marked_source(fields: &Fields, span: &Ident) -> syn::Result<Option<TokenStream>> {
    let list = field_list(fields);
    let mut marked = list.iter().enumerate().filter(|(_, f)| {
        f.attrs.iter().any(|a| a.path().is_ident("source"))
            || f.ident.as_ref().is_some_and(|i| i == "source")
    });
    match (marked.next(), marked.next()) {
        (None, _) => Ok(None),
        (Some((i, _)), None) => {
            let name = field_binding(fields, i);
            Ok(Some(quote! { #name }))
        }
        (Some(_), Some(_)) => Err(syn::Error::new_spanned(
            span,
            "more than one `#[source]`/`source` field; errlanes picks at most one",
        )),
    }
}

/// Pulls the single `#[error("..")]` off a unit's attributes, erroring on a
/// second one. Every other attribute is left untouched.
pub(crate) fn take_error_lit(attrs: &[syn::Attribute]) -> syn::Result<Option<LitStr>> {
    let mut found = None;
    for attr in attrs {
        if attr.path().is_ident("error") {
            if found.is_some() {
                return Err(syn::Error::new_spanned(
                    attr,
                    "at most one #[error(\"..\")] per type or variant",
                ));
            }
            found = Some(attr.parse_args::<LitStr>()?);
        }
    }
    Ok(found)
}

/// `#[from]` on a field is `thiserror`'s grammar, not errlanes': point at
/// the replacement instead of silently accepting or ignoring it.
pub(crate) fn reject_from_field_attr(fields: &Fields) -> syn::Result<()> {
    for field in field_list(fields) {
        if field.attrs.iter().any(|a| a.path().is_ident("from")) {
            return Err(syn::Error::new_spanned(
                field,
                "`#[from]` is not supported here; use `from` inside `#[classify(..)]` / \
                 `#[rejection(..)]` instead, or `error = manual` to let thiserror own this type",
            ));
        }
    }
    Ok(())
}

/// A field referenced by an `#[error("..")]` placeholder: its bound name,
/// type, and whether the placeholder requested `Debug` (`:?`).
type FieldRef = (Ident, Type, bool);

/// Rewrites positional placeholders (`{0}`, `{1:?}`) in an `#[error("..")]`
/// literal to the referenced field's bound local name, so it resolves as a
/// Rust 2021 inline capture; a named placeholder (`{field}`) already
/// resolves natively and is only validated here. Returns the rewritten
/// literal text plus, for each referenced field, its bound name, type, and
/// whether its format spec requested `Debug` (contains `?`).
fn rewrite_literal(
    lit: &LitStr,
    fields: &Fields,
) -> syn::Result<(String, Vec<FieldRef>)> {
    let names = bound_fields(fields);
    let types: Vec<Type> = field_list(fields).iter().map(|f| f.ty.clone()).collect();
    let src = lit.value();
    let mut out = String::with_capacity(src.len());
    let mut refs = Vec::new();
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push_str("{{");
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push_str("}}");
            }
            '{' => {
                let mut body = String::new();
                let mut closed = false;
                for c2 in chars.by_ref() {
                    if c2 == '}' {
                        closed = true;
                        break;
                    }
                    body.push(c2);
                }
                if !closed {
                    return Err(syn::Error::new(
                        lit.span(),
                        "unterminated `{` in #[error(\"..\")]",
                    ));
                }
                let (name_part, spec) = match body.find(':') {
                    Some(i) => (&body[..i], &body[i..]),
                    None => (body.as_str(), ""),
                };
                if name_part.is_empty() {
                    return Err(syn::Error::new(
                        lit.span(),
                        "#[error(\"..\")] does not support auto-indexed `{}`; name the field, \
                         e.g. `{0}` or `{field}`",
                    ));
                }
                let is_debug = spec.contains('?');
                if let Ok(index) = name_part.parse::<usize>() {
                    let Some(name) = names.get(index) else {
                        return Err(syn::Error::new(
                            lit.span(),
                            format!(
                                "#[error(\"..\")] references field {index}, but this has {} \
                                 field(s)",
                                names.len()
                            ),
                        ));
                    };
                    refs.push((name.clone(), types[index].clone(), is_debug));
                    out.push('{');
                    out.push_str(&name.to_string());
                    out.push_str(spec);
                    out.push('}');
                } else {
                    let Some(index) = names.iter().position(|n| n == name_part) else {
                        return Err(syn::Error::new(
                            lit.span(),
                            format!(
                                "#[error(\"..\")] references `{{{name_part}}}`, which is not a \
                                 field here"
                            ),
                        ));
                    };
                    refs.push((names[index].clone(), types[index].clone(), is_debug));
                    out.push('{');
                    out.push_str(&body);
                    out.push('}');
                }
            }
            other => out.push(other),
        }
    }
    Ok((out, refs))
}

fn type_mentions_param(ty: &Type, param: &Ident) -> bool {
    ty.to_token_stream()
        .into_iter()
        .any(|t| matches!(t, proc_macro2::TokenTree::Ident(i) if i == *param))
}

/// A field referenced by an `#[error(..)]` placeholder whose type mentions
/// one of the deriving type's own generic parameters needs an explicit
/// `Display`/`Debug` bound on the generated impl — the type parameter has
/// no such bound otherwise.
fn extra_bounds(generics: &syn::Generics, refs: &[FieldRef]) -> Vec<TokenStream> {
    let params: Vec<Ident> = generics.type_params().map(|p| p.ident.clone()).collect();
    if params.is_empty() {
        return Vec::new();
    }
    let mut bounds: Vec<(String, TokenStream)> = Vec::new();
    for (_, ty, is_debug) in refs {
        if !params.iter().any(|p| type_mentions_param(ty, p)) {
            continue;
        }
        let bound = if *is_debug {
            quote! { #ty: std::fmt::Debug }
        } else {
            quote! { #ty: std::fmt::Display }
        };
        let key = bound.to_string();
        if !bounds.iter().any(|(k, _)| *k == key) {
            bounds.push((key, bound));
        }
    }
    bounds.into_iter().map(|(_, b)| b).collect()
}

/// Emits `impl Display` and `impl std::error::Error` for `target`, one match
/// arm per unit.
pub(crate) fn emit(
    target: &Ident,
    generics: &syn::Generics,
    default: DisplayDefault,
    units: &[Unit<'_>],
) -> syn::Result<TokenStream> {
    let mut display_arms = Vec::with_capacity(units.len());
    let mut source_arms = Vec::with_capacity(units.len());
    let mut all_refs = Vec::new();
    for unit in units {
        let pat = &unit.pat;
        let body = match &unit.error {
            Some(lit) => {
                let (rewritten, refs) = rewrite_literal(lit, unit.fields)?;
                all_refs.extend(refs);
                // `call_site()`, not `lit.span()`: a composed type's `#[error(..)]`
                // can arrive via a macro round-trip (the composition schema's
                // `macro_rules!`), which gives the literal a different hygiene
                // context than the `field_N` idents this invocation just bound —
                // an inline capture resolves by the format string's own span, so
                // a mismatch here reads as "cannot find value" despite the name
                // matching.
                let rewritten = LitStr::new(&rewritten, proc_macro2::Span::call_site());
                quote! { write!(f, #rewritten) }
            }
            None => match default {
                DisplayDefault::Code => {
                    quote! { f.write_str(<Self as errlanes::Rejection>::code(self).into()) }
                }
                DisplayDefault::Name => {
                    let name = unit.name.to_string().to_case(Case::Snake);
                    quote! { f.write_str(#name) }
                }
            },
        };
        display_arms.push(quote! { #pat => #body });
        let source_body = match &unit.source {
            Some(name) => quote! { Some(#name as &(dyn std::error::Error + 'static)) },
            None => quote! { None },
        };
        source_arms.push(quote! { #pat => #source_body });
    }
    let extra = extra_bounds(generics, &all_refs);
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let where_tokens = if extra.is_empty() {
        quote! { #where_clause }
    } else if let Some(wc) = where_clause {
        quote! { #wc #(, #extra)* }
    } else {
        quote! { where #(#extra),* }
    };
    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics std::fmt::Display for #target #ty_generics #where_tokens {
            #[allow(unused_variables)]
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    #(#display_arms,)*
                }
            }
        }
        #[automatically_derived]
        impl #impl_generics std::error::Error for #target #ty_generics #where_tokens {
            #[allow(unused_variables)]
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                match self {
                    #(#source_arms,)*
                }
            }
        }
    })
}
