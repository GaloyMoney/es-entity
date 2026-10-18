use std::collections::{HashMap, HashSet};

use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use syn::{
    Ident, ItemEnum, Path, Token, Variant, parse::Parse, parse_quote, punctuated::Punctuated,
};

type Variants = Punctuated<Variant, Token![,]>;

const MODIFIERS: [&str; 1] = ["no_from"];

/// One entry of `#[errlanes::compose(Source, Other as Prefix)]`. A bare path
/// imports the family under its original variant names; `as Prefix` keeps its
/// cases distinct by prefixing every imported name.
struct Import {
    path: Path,
    prefix: Option<Ident>,
    no_from: bool,
}

impl Parse for Import {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let path: Path = input.parse()?;
        let prefix = if input.peek(Token![as]) {
            input.parse::<Token![as]>()?;
            Some(input.parse()?)
        } else {
            None
        };
        let mut no_from = false;
        if input.peek(syn::token::Paren) {
            let group;
            let paren = syn::parenthesized!(group in input);
            if group.is_empty() {
                return Err(syn::Error::new(
                    paren.span.join(),
                    "empty modifier list; drop the parentheses",
                ));
            }
            for name in group.parse_terminated(Ident::parse, Token![,])? {
                if name != "no_from" {
                    return Err(syn::Error::new_spanned(name, "expected `no_from`"));
                }
                if no_from {
                    return Err(syn::Error::new_spanned(name, "duplicate `no_from`"));
                }
                no_from = true;
            }
            if input.peek(Token![as]) {
                return Err(input.error("modifiers follow the prefix: `Source as Prefix(no_from)`"));
            }
        }
        Ok(Self {
            path,
            prefix,
            no_from,
        })
    }
}

// The import list travels through the schema callback as tokens, so it has to
// round-trip through its own `Parse`.
impl ToTokens for Import {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        self.path.to_tokens(tokens);
        if let Some(prefix) = &self.prefix {
            tokens.extend(quote!(as #prefix));
        }
        if self.no_from {
            tokens.extend(quote!((no_from)));
        }
    }
}

struct Arguments {
    imports: Punctuated<Import, Token![,]>,
}

impl Parse for Arguments {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let imports = input.parse_terminated(Import::parse, Token![,])?;
        if imports.is_empty() {
            return Err(input.error("compose requires at least one source enum"));
        }
        // A modifier written as its own argument would otherwise parse as a
        // source path and fail later on a missing helper macro.
        for (index, import) in imports.iter().enumerate() {
            if index > 0
                && import.prefix.is_none()
                && !import.no_from
                && let Some(name) = import.path.get_ident()
                && MODIFIERS.iter().any(|modifier| name == modifier)
            {
                return Err(syn::Error::new_spanned(
                    name,
                    format!("`{name}` is an import modifier; write `Source({name})`"),
                ));
            }
        }
        Ok(Self { imports })
    }
}

struct Source {
    path: Path,
    prefix: Option<Ident>,
    no_from: bool,
    ids: Vec<u64>,
    variants: Variants,
}

// Collect every schema before resolving names. This makes merges independent
// of source order and lets explicit participants be checked against real cases.
pub(super) struct Context {
    item: ItemEnum,
    remaining: Punctuated<Import, Token![,]>,
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
            let prefix;
            syn::bracketed!(prefix in sources);
            let no_from;
            syn::bracketed!(no_from in sources);
            let ids;
            syn::bracketed!(ids in sources);
            let variants;
            syn::braced!(variants in sources);
            collected.push(Source {
                path: path.parse()?,
                prefix: if prefix.is_empty() {
                    None
                } else {
                    Some(prefix.parse()?)
                },
                no_from: !no_from.is_empty(),
                ids: ids
                    .parse_terminated(syn::LitInt::parse, Token![,])?
                    .iter()
                    .map(|id| id.base10_parse())
                    .collect::<syn::Result<_>>()?,
                variants: variants.parse_terminated(Variant::parse, Token![,])?,
            });
        }
        Ok(Self {
            item: item.parse()?,
            remaining: remaining.parse_terminated(Import::parse, Token![,])?,
            sources: collected,
        })
    }
}

pub(super) fn start(args: TokenStream, item: ItemEnum) -> syn::Result<TokenStream> {
    let Arguments { imports } = syn::parse2(args)?;
    let mut seen = HashSet::new();
    for import in &imports {
        if !seen.insert(key(&import.path)) {
            return Err(syn::Error::new_spanned(
                import,
                "duplicate composition source",
            ));
        }
        if import
            .path
            .segments
            .iter()
            .any(|segment| !matches!(segment.arguments, syn::PathArguments::None))
        {
            return Err(syn::Error::new_spanned(
                &import.path,
                "generic rejection composition is unsupported; use a concrete enum",
            ));
        }
        for attr in &item.attrs {
            if attr.path().is_ident("lift")
                && crate::lift::registration(attr)?
                    .is_some_and(|registration| key(&registration.source) == key(&import.path))
            {
                return Err(syn::Error::new_spanned(
                    attr,
                    "source family already mapped; list it in #[errlanes::compose(..)] or map it with #[lift], not both",
                ));
            }
        }
    }
    Context {
        item,
        remaining: imports,
        sources: Vec::new(),
    }
    .next()
}

impl Context {
    fn next(self) -> syn::Result<TokenStream> {
        if self.remaining.is_empty() {
            return finish(self.item, self.sources);
        }
        let source = &self.remaining.first().unwrap().path;
        let mut helper = source.clone();
        let last = helper.segments.last_mut().unwrap();
        last.ident = format_ident!("{}Schema", last.ident);
        let item = &self.item;
        let remaining = &self.remaining;
        let sources = self.sources.iter().map(
            |Source {
                 path,
                 prefix,
                 no_from,
                 ids,
                 variants,
             }| {
                let no_from = no_from.then(|| quote!(no_from));
                quote! { [#path] [#prefix] [#no_from] [#(#ids),*] { #variants } }
            },
        );
        Ok(quote! {
            #helper!(errlanes::__compose_rejection,
                [[#item] [#remaining] [#(#sources)*]], #source);
        })
    }

    pub(super) fn receive(mut self, variants: Variants, ids: Vec<u64>) -> syn::Result<TokenStream> {
        let Import {
            path,
            prefix,
            no_from,
        } = self.remaining.first().unwrap();
        self.sources.push(Source {
            path: path.clone(),
            prefix: prefix.clone(),
            no_from: *no_from,
            ids,
            variants,
        });
        self.remaining = self.remaining.into_iter().skip(1).collect();
        self.next()
    }
}

pub(super) fn key(value: &impl ToTokens) -> String {
    value.to_token_stream().to_string()
}

/// A `#[compose(..)]` that survives to the end of composition is either the
/// removed placeholder form or a merge with no sources to merge.
pub(super) fn unexpected(attr: &syn::Attribute) -> syn::Error {
    if attr
        .parse_args::<Ident>()
        .is_ok_and(|name| name == "flatten")
    {
        return removed(attr);
    }
    syn::Error::new_spanned(
        attr,
        "#[compose(merge)] needs sources to merge; list them as #[errlanes::compose(Source, Other as Prefix)]",
    )
}

fn removed(spanned: impl ToTokens) -> syn::Error {
    syn::Error::new_spanned(
        spanned,
        "whole-family imports are listed on the attribute: write #[errlanes::compose(Source as Prefix)] and drop this placeholder variant",
    )
}

enum Merge {
    ByName,
    Explicit(Punctuated<Path, Token![,]>),
}

impl Parse for Merge {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let name: Ident = input.parse()?;
        if name == "flatten" {
            return Err(removed(name));
        }
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
    for Source { path, variants, .. } in &sources {
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
                            "merge participant is not a variant of a listed composition source",
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
    let mut collected = Vec::new();
    // A source the composing enum already converts from, by its own
    // spelling, is not re-exported through an import that also reaches it.
    let mut covered = HashSet::new();
    for attr in &item.attrs {
        if attr.path().is_ident("lift")
            && let Some(registration) = crate::lift::registration(attr)?
        {
            covered.insert(super::variant_id(&key(&registration.source)));
        }
    }
    for source in &sources {
        covered.insert(super::variant_id(&key(&source.path)));
    }
    let mut vias = Vec::new();
    for Source {
        path,
        prefix,
        no_from,
        ids,
        variants,
    } in sources
    {
        if !no_from {
            for id in ids {
                if covered.insert(id) {
                    vias.push(crate::lift::Via {
                        via: path.clone(),
                        id,
                    });
                }
            }
        }
        for mut variant in variants {
            let name = variant.ident.clone();
            let case: Path = parse_quote!(#path::#name);
            // A prefix exists to keep a family's cases distinct, so it never
            // joins a by-name merge; an explicit participant list still can.
            let by_name_target = prefix
                .is_none()
                .then(|| by_name.get(&name.to_string()).copied())
                .flatten();
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
            let imported = match &prefix {
                Some(prefix) => format_ident!("{prefix}{name}"),
                None => name,
            };
            let destination = target
                .map(|i| item.variants[i].ident.to_string())
                .unwrap_or_else(|| imported.to_string());
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
                        "composed variant name collision; declare a local compose(merge) variant, list explicit merge participants, or import the source under an `as` prefix",
                    ));
                }
                variant.ident = imported;
                variant.attrs.push(parse_quote!(#[lift(#case)]));
                variant
                    .attrs
                    .push(parse_quote!(#[rejection(code_and_level_from = #case)]));
                collected.push(variant);
            }
        }
        item.attrs.push(parse_quote!(#[lift(#path, strict)]));
    }
    for via in vias {
        item.attrs.push(parse_quote!(#[lift(#via)]));
    }
    for index in merges {
        if !used.contains(&index) {
            return Err(syn::Error::new_spanned(
                &item.variants[index],
                "compose(merge) did not match any composition source variant",
            ));
        }
    }
    item.variants.extend(collected);
    super::expand(item)
}
