//! Shared `std::fmt::Display` + `std::error::Error` emission for
//! `#[derive(errlanes::Rejection)]` and `#[derive(errlanes::Classify)]`. See
//! the `errlanes-native-display-error` handoff for the design this
//! implements: a rejection displays its code, a laned wrapper displays its
//! snake_case name, and both read `source()` off the same field marker rule
//! (`#[source]` / a field named `source` / a `delegate` or `from` payload).

use convert_case::{Case, Casing};
use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use syn::{
    Expr, Fields, Ident, LitStr, Token, Type,
    parse::{Parse, ParseStream},
};

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
    pub error: Option<ErrorSpec>,
    /// This unit's own name, used by the `Name` default.
    pub name: Ident,
    /// The locally-bound field name that is `source()`, if any.
    pub source: Option<TokenStream>,
}

/// `#[error("..", trailing, args)]`: the format string, plus any
/// `thiserror`-style trailing format arguments. Each trailing argument must
/// be a field access path (a bound field name, optionally followed by one or
/// more `.member` accesses, e.g. `failure.error`) — see [`Parse`] below for
/// why arbitrary expressions are rejected here rather than accepted.
#[derive(Clone)]
pub(crate) struct ErrorSpec {
    pub lit: LitStr,
    pub args: Vec<Expr>,
}

/// Only a bound field name, or a chain of `.member` accesses off one, is
/// accepted as a trailing argument. This is deliberately narrower than
/// `thiserror`, which accepts arbitrary expressions: extending `#[error(..)]`
/// to arbitrary Rust expressions would let a derive attribute run arbitrary
/// code, and (per the design handoff this implements) the field-of-a-field
/// shape is all the motivating case needs. The root is checked against the
/// unit's actual fields later, once a [`Fields`] is in scope (see
/// `check_trailing_args`); here only the *shape* is validated.
fn validate_field_access_path(expr: &Expr) -> syn::Result<()> {
    match expr {
        Expr::Path(p) if p.path.get_ident().is_some() => Ok(()),
        Expr::Field(f) => validate_field_access_path(&f.base),
        _ => Err(syn::Error::new_spanned(
            expr,
            "#[error(\"..\", ..)] trailing arguments must be field access paths (e.g. \
             `field.sub_field`); arbitrary expressions are not supported",
        )),
    }
}

impl Parse for ErrorSpec {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let lit: LitStr = input.parse()?;
        let mut args = Vec::new();
        while !input.is_empty() {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                // A single trailing comma, same as `write!`/`format!`.
                break;
            }
            let arg = input.parse::<Expr>()?;
            validate_field_access_path(&arg)?;
            args.push(arg);
        }
        Ok(ErrorSpec { lit, args })
    }
}

/// The bound local an already-shape-validated field access path is rooted
/// at. Panics on a path that `validate_field_access_path` would have
/// rejected — every [`ErrorSpec::args`] entry is validated at parse time, so
/// this is an invariant, not user input.
fn root_ident(expr: &Expr) -> &Ident {
    match expr {
        Expr::Path(p) => p.path.get_ident().expect("validated as a bare ident"),
        Expr::Field(f) => root_ident(&f.base),
        _ => unreachable!("ErrorSpec::args are validated to be field access paths"),
    }
}

/// Checks that every trailing argument's root is one of this unit's own
/// fields — the same diagnostic shape `rewrite_literal` uses for named
/// placeholders that are not a field.
fn check_trailing_args(args: &[Expr], fields: &Fields) -> syn::Result<()> {
    let names = bound_fields(fields);
    for arg in args {
        let root = root_ident(arg);
        if !names.iter().any(|n| n == root) {
            return Err(syn::Error::new_spanned(
                root,
                format!(
                    "#[error(\"..\", ..)] trailing argument starts from `{root}`, which is not \
                     a field here"
                ),
            ));
        }
    }
    Ok(())
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
pub(crate) fn take_error_lit(attrs: &[syn::Attribute]) -> syn::Result<Option<ErrorSpec>> {
    let mut found = None;
    for attr in attrs {
        if attr.path().is_ident("error") {
            if found.is_some() {
                return Err(syn::Error::new_spanned(
                    attr,
                    "at most one #[error(\"..\")] per type or variant",
                ));
            }
            found = Some(attr.parse_args::<ErrorSpec>()?);
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
/// literal to the referenced field's bound local name; a named placeholder
/// (`{field}`) is only validated here. The emitter passes those bindings as
/// explicit named format arguments. Returns the rewritten
/// literal text plus, for each referenced field, its bound name, type, and
/// whether its format spec requested `Debug` (contains `?`).
///
/// `allow_auto_index` is `true` only when the `#[error(..)]` carries trailing
/// format arguments: auto-indexed `{}` is then passed through unchanged, to
/// be matched positionally against those arguments by the emitted `write!`,
/// exactly as `write!`/`format!` already do when mixing named and positional
/// arguments. With no trailing arguments, `{}` stays rejected with today's
/// diagnostic — unchanged, since nothing before it could ever have supplied
/// a positional argument for `{}` to bind to.
fn rewrite_literal(
    lit: &LitStr,
    fields: &Fields,
    allow_auto_index: bool,
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
                    if allow_auto_index {
                        out.push('{');
                        out.push_str(&body);
                        out.push('}');
                        continue;
                    }
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

/// Whether `param` appears anywhere in `ty`'s tokens — recursing into
/// groups (`(..)`, `[..]`, `{..}`), not just the top level, so a param
/// buried in a tuple, array, or parenthesized type (`(E, String)`, `[E; 3]`)
/// is still found. A flat, one-level scan over `to_token_stream()` would
/// see such a group as a single opaque `TokenTree::Group` and miss the
/// `Ident` inside it entirely.
fn type_mentions_param(ty: &Type, param: &Ident) -> bool {
    fn contains(ts: TokenStream, param: &Ident) -> bool {
        ts.into_iter().any(|t| match t {
            proc_macro2::TokenTree::Ident(i) => i == *param,
            proc_macro2::TokenTree::Group(g) => contains(g.stream(), param),
            _ => false,
        })
    }
    contains(ty.to_token_stream(), param)
}

/// Every field's own declared type, in declaration order. Unlike
/// [`FieldRef`], this isn't limited to fields an `#[error(..)]` placeholder
/// happens to reference — a caller needing a bound that applies to the
/// *whole* value (e.g. `classify_bounds`, which mirrors what `Debug`'s own
/// derive and the `Send`/`Sync` auto traits already require structurally)
/// needs every field, formatted or not.
pub(crate) fn field_types(fields: &Fields) -> Vec<Type> {
    field_list(fields).iter().map(|f| f.ty.clone()).collect()
}

/// Appends `extra` bounds to `where_clause`, synthesizing a fresh `where`
/// when there wasn't one. Shared by [`emit`]'s own `Display`/`Error` impls
/// and by `classify.rs`'s generated `Classify` impl, which needs the
/// analogous [`classify_bounds`] appended the same way.
pub(crate) fn merge_where(
    where_clause: Option<&syn::WhereClause>,
    extra: &[TokenStream],
) -> TokenStream {
    if extra.is_empty() {
        quote! { #where_clause }
    } else if let Some(wc) = where_clause {
        quote! { #wc #(, #extra)* }
    } else {
        quote! { where #(#extra),* }
    }
}

/// The *structural* share of what the generated `Classify` impl needs for
/// its own supertrait (`Classify: Error + Send + Sync + 'static`) to hold
/// when the deriving type is generic: every field, formatted or not,
/// contributes to `Self: Debug` (the standard `#[derive(Debug)]`'s own
/// bound) and to `Self: Send + Sync + 'static` (the auto traits), so every
/// generic parameter that appears in any field needs all four. Unlike
/// [`extra_bounds`] — which binds a *formatted* field's own compound type to
/// whichever of `Display`/`Debug` its placeholder asked for — this binds the
/// bare type *parameter*: `Debug`'s own derive and the auto traits already
/// propagate through any container (`Vec<E>`, `Option<E>`, …) once `E`
/// itself carries the bound, so the parameter is both simpler and
/// sufficient here.
///
/// This is only the structural share: `Self: Error` also needs `Self:
/// Display`, which — for a generic field — only this derive's own
/// `std_error::emit` can know about (it depends on which fields a message
/// actually formats, and whether `:?` was used). The caller must still
/// merge in the `extra` bounds `emit` returns alongside its `Display`/
/// `Error` impls; `classify_bounds` alone is not sufficient whenever a
/// non-`Debug` placeholder formats a field whose type mentions a generic
/// parameter directly (`#[error("{0}")]` over a bare `E` field, say).
pub(crate) fn classify_bounds(generics: &syn::Generics, field_types: &[Type]) -> Vec<TokenStream> {
    generics
        .type_params()
        .map(|p| &p.ident)
        .filter(|p| field_types.iter().any(|ty| type_mentions_param(ty, p)))
        .map(|p| quote! { #p: std::fmt::Debug + Send + Sync + 'static })
        .collect()
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
/// arm per unit, plus the `extra_bounds` this emission needed — a caller
/// that also emits a `Classify` impl for the same generic type (`Classify:
/// Error + ..`) needs these same bounds merged into its own where-clause,
/// since they are exactly what makes `Self: Display`/`Self: Error` hold
/// here; see `classify_bounds`'s doc comment.
pub(crate) fn emit(
    target: &Ident,
    generics: &syn::Generics,
    default: DisplayDefault,
    units: &[Unit<'_>],
) -> syn::Result<(TokenStream, Vec<TokenStream>)> {
    let mut display_arms = Vec::with_capacity(units.len());
    let mut source_arms = Vec::with_capacity(units.len());
    let mut all_refs = Vec::new();
    for unit in units {
        let pat = &unit.pat;
        let body = match &unit.error {
            Some(spec) => {
                check_trailing_args(&spec.args, unit.fields)?;
                let (rewritten, refs) =
                    rewrite_literal(&spec.lit, unit.fields, !spec.args.is_empty())?;
                let mut named_args = Vec::new();
                for (name, _, _) in &refs {
                    if !named_args.contains(name) {
                        named_args.push(name.clone());
                    }
                }
                all_refs.extend(refs);
                // Explicit arguments retain the bound fields' hygiene even
                // when local and imported variants have crossed different
                // schema callbacks. No single format-literal span can make
                // implicit captures resolve correctly for both.
                let rewritten = LitStr::new(&rewritten, spec.lit.span());
                let args = &spec.args;
                // The trailing arguments are not field-only placeholders, so
                // they get no `FieldRef` and no entry in `extra_bounds` — the
                // expression's own type is whatever it is at the `write!`
                // call site, and rustc infers the `Display`/`Debug` bound
                // from that call directly. Extracting a `syn::Type` for an
                // arbitrary expression is not generally possible, so this
                // path deliberately does not try; only the field-only
                // placeholders above feed `extra_bounds`.
                quote! { write!(f, #rewritten #(, #args)* #(, #named_args = #named_args)*) }
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
    // `std::error::Error: Debug + Display` — unconditionally, regardless of
    // which fields (if any) a message actually formats. `extra_bounds`
    // above only ever adds a bound for a field some placeholder *formats*,
    // which is right for `Display` (nothing else needs that field rendered)
    // but not sufficient for `Error`'s own `Debug` supertrait: that needs
    // every field's type to be `Debug`, exactly like the standard
    // `#[derive(Debug)]`'s own bound (`P: Debug` per generic parameter,
    // structurally, over every field) — the same criterion `classify_bounds`
    // already uses. Computed separately from `extra` because it applies
    // only to the `Error` impl below, not the `Display` impl, which needs
    // nothing beyond what a placeholder actually references.
    let all_field_types: Vec<Type> = units.iter().flat_map(|u| field_types(u.fields)).collect();
    let mut error_extra = extra.clone();
    for param in generics.type_params().map(|p| &p.ident) {
        if !all_field_types
            .iter()
            .any(|ty| type_mentions_param(ty, param))
        {
            continue;
        }
        let bound = quote! { #param: std::fmt::Debug };
        let key = bound.to_string();
        if !error_extra.iter().any(|b| b.to_string() == key) {
            error_extra.push(bound);
        }
    }
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let display_where_tokens = merge_where(where_clause, &extra);
    let error_where_tokens = merge_where(where_clause, &error_extra);
    let tokens = quote! {
        #[automatically_derived]
        impl #impl_generics std::fmt::Display for #target #ty_generics #display_where_tokens {
            #[allow(unused_variables)]
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    #(#display_arms,)*
                }
            }
        }
        #[automatically_derived]
        impl #impl_generics std::error::Error for #target #ty_generics #error_where_tokens {
            #[allow(unused_variables)]
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                match self {
                    #(#source_arms,)*
                }
            }
        }
    };
    Ok((tokens, extra))
}
