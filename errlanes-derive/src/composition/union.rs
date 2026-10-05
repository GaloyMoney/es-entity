use std::collections::{HashMap, HashSet};

use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use syn::{ItemEnum, Path, Token, Variant, parse::Parse, parse_quote, punctuated::Punctuated};

type Variants = Punctuated<Variant, Token![,]>;

struct Arguments {
    sources: Punctuated<Path, Token![,]>,
}

impl Parse for Arguments {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        input.parse::<Token![union]>()?;
        let sources;
        syn::parenthesized!(sources in input);
        let sources = sources.parse_terminated(Path::parse, Token![,])?;
        if sources.is_empty() {
            return Err(input.error("union requires at least one source enum"));
        }
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        }
        Ok(Self { sources })
    }
}

struct Source {
    path: Path,
    variants: Variants,
}

// Collect every schema before resolving names. This makes merges independent
// of source order and lets explicit participants be checked against real cases.
pub(super) struct Context {
    item: ItemEnum,
    remaining: Punctuated<Path, Token![,]>,
    sources: Vec<Source>,
}

impl Parse for Context {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let item;
        syn::bracketed!(item in input);
        let remaining;
        syn::bracketed!(remaining in input);
        let sources;
        syn::bracketed!(sources in input);
        let mut collected = Vec::new();
        while !sources.is_empty() {
            let path;
            syn::bracketed!(path in sources);
            let variants;
            syn::braced!(variants in sources);
            collected.push(Source {
                path: path.parse()?,
                variants: variants.parse_terminated(Variant::parse, Token![,])?,
            });
        }
        Ok(Self {
            item: item.parse()?,
            remaining: remaining.parse_terminated(Path::parse, Token![,])?,
            sources: collected,
        })
    }
}

pub(super) fn start(args: TokenStream, item: ItemEnum) -> syn::Result<TokenStream> {
    let Arguments { sources } = syn::parse2(args)?;
    let mut seen = HashSet::new();
    for source in &sources {
        if !seen.insert(key(source)) {
            return Err(syn::Error::new_spanned(source, "duplicate union source"));
        }
        if source
            .segments
            .iter()
            .any(|segment| !matches!(segment.arguments, syn::PathArguments::None))
        {
            return Err(syn::Error::new_spanned(
                source,
                "generic rejection composition is unsupported; use a concrete enum",
            ));
        }
        for attr in &item.attrs {
            if attr.path().is_ident("lift")
                && key(&attr.parse_args::<crate::lift::Registration>()?.source) == key(source)
            {
                return Err(syn::Error::new_spanned(
                    attr,
                    "union source family already mapped; use compose(merge) to resolve its variants",
                ));
            }
        }
    }
    Context {
        item,
        remaining: sources,
        sources: Vec::new(),
    }
    .next()
}

impl Context {
    fn next(self) -> syn::Result<TokenStream> {
        if self.remaining.is_empty() {
            return finish(self.item, self.sources);
        }
        let source = self.remaining.first().unwrap().clone();
        let mut helper = source.clone();
        let last = helper.segments.last_mut().unwrap();
        last.ident = format_ident!("{}Schema", last.ident);
        let item = &self.item;
        let remaining = &self.remaining;
        let sources = self.sources.iter().map(|Source { path, variants }| {
            quote! { [#path] { #variants } }
        });
        Ok(quote! {
            #helper!(errlanes::__compose_rejection,
                [union [#item] [#remaining] [#(#sources)*]], #source);
        })
    }

    pub(super) fn receive(mut self, variants: Variants) -> syn::Result<TokenStream> {
        let source = self.remaining.first().unwrap().clone();
        self.remaining = self.remaining.into_iter().skip(1).collect();
        self.sources.push(Source {
            path: source,
            variants,
        });
        self.next()
    }
}

fn key(value: &impl ToTokens) -> String {
    value.to_token_stream().to_string()
}

enum Merge {
    ByName,
    Explicit(Punctuated<Path, Token![,]>),
}

impl Parse for Merge {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let name: syn::Ident = input.parse()?;
        if name != "merge" {
            return Err(syn::Error::new_spanned(
                name,
                "expected compose(merge) or compose(merge(Source::Variant, ...))",
            ));
        }
        if input.is_empty() {
            return Ok(Self::ByName);
        }
        let cases;
        syn::parenthesized!(cases in input);
        let cases = cases.parse_terminated(Path::parse, Token![,])?;
        if cases.is_empty() {
            return Err(input.error("explicit merge requires at least one source variant"));
        }
        Ok(Self::Explicit(cases))
    }
}

fn finish(mut item: ItemEnum, sources: Vec<Source>) -> syn::Result<TokenStream> {
    let mut available = HashSet::new();
    for Source { path, variants } in &sources {
        for variant in variants {
            let name = &variant.ident;
            available.insert(key(&quote!(#path::#name)));
        }
    }
    let mut by_name = HashMap::new();
    let mut explicit = HashMap::new();
    let mut merges = Vec::new();
    for (index, variant) in item.variants.iter_mut().enumerate() {
        let attrs: Vec<_> = variant
            .attrs
            .iter()
            .filter(|a| a.path().is_ident("compose"))
            .collect();
        // Ordinary prefixed imports can coexist with a union.
        if attrs.len() == 1
            && attrs[0]
                .parse_args::<syn::Ident>()
                .is_ok_and(|name| name == "flatten")
        {
            continue;
        }
        if attrs.is_empty() {
            continue;
        }
        if attrs.len() != 1 {
            return Err(syn::Error::new_spanned(
                variant,
                "expected exactly one compose attribute per variant",
            ));
        }
        let attr = attrs[0];
        match attr.parse_args::<Merge>()? {
            Merge::ByName => {
                by_name.insert(variant.ident.to_string(), index);
            }
            Merge::Explicit(cases) => {
                for case in cases {
                    let case_key = key(&case);
                    if !available.contains(&case_key) {
                        return Err(syn::Error::new_spanned(
                            case,
                            "merge participant is not a variant of a listed union source",
                        ));
                    }
                    if explicit.insert(case_key, index).is_some() {
                        return Err(syn::Error::new_spanned(
                            case,
                            "source variant merged more than once",
                        ));
                    }
                }
            }
        }
        // A merge is a semantic choice: metadata belongs to the local
        // declaration, never to whichever source happened to be visited first.
        let mut canonical = false;
        for attr in &variant.attrs {
            if attr.path().is_ident("rejection") {
                attr.parse_nested_meta(|meta| {
                    canonical |= meta.path.is_ident("code") || meta.path.is_ident("delegate");
                    if meta.input.peek(Token![=]) {
                        let _: syn::Expr = meta.value()?.parse()?;
                    }
                    Ok(())
                })?;
            }
        }
        if !canonical {
            return Err(syn::Error::new_spanned(
                variant,
                "merged variants require an explicit canonical rejection code or rejection(delegate)",
            ));
        }
        variant
            .attrs
            .retain(|attr| !attr.path().is_ident("compose"));
        merges.push(index);
    }

    let mut used = HashSet::new();
    let mut names: HashSet<_> = item.variants.iter().map(|v| v.ident.to_string()).collect();
    let mut origins = HashMap::new();
    let mut imported = Vec::new();
    for Source { path, variants } in sources {
        for mut variant in variants {
            let name = &variant.ident;
            let case: Path = parse_quote!(#path::#name);
            let by_name_target = by_name.get(&name.to_string()).copied();
            let explicit_target = explicit.get(&key(&case)).copied();
            if by_name_target.is_some()
                && explicit_target.is_some()
                && by_name_target != explicit_target
            {
                return Err(syn::Error::new_spanned(
                    case,
                    "source variant selected by both a name merge and an explicit merge",
                ));
            }
            let target = explicit_target.or(by_name_target);
            let destination = target
                .map(|i| item.variants[i].ident.to_string())
                .unwrap_or_else(|| name.to_string());
            if let Some(origin) = super::origin(&variant)?
                && let Some(previous) = origins.insert(origin, destination.clone())
                && previous != destination
            {
                return Err(syn::Error::new_spanned(
                    case,
                    "the same rejection leaf arrives at multiple destinations; merge explicitly to a canonical destination",
                ));
            }
            if let Some(index) = target {
                item.variants[index]
                    .attrs
                    .push(parse_quote!(#[lift(#case)]));
                used.insert(index);
            } else {
                if !names.insert(destination) {
                    return Err(syn::Error::new_spanned(
                        case,
                        "union variant name collision; declare a local compose(merge) variant or list explicit merge participants",
                    ));
                }
                variant.attrs.push(parse_quote!(#[lift(#case)]));
                variant
                    .attrs
                    .push(parse_quote!(#[rejection(forward = #case)]));
                imported.push(variant);
            }
        }
        item.attrs.push(parse_quote!(#[lift(#path, strict)]));
    }
    for index in merges {
        if !used.contains(&index) {
            return Err(syn::Error::new_spanned(
                &item.variants[index],
                "compose(merge) did not match any union source variant",
            ));
        }
    }
    item.variants.extend(imported);
    super::expand(item)
}
