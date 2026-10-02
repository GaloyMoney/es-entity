use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use syn::{ItemEnum, Token, parse::Parse, parse_quote};

// Stable protocol IDs, independent of the process and dependency name.
pub(crate) fn variant_id(name: &str) -> u64 {
    name.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}

pub fn schema(input: &syn::DeriveInput) -> syn::Result<TokenStream> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "Rejection composition supports monomorphic enums; use concrete payload types or a concrete enum facade",
        ));
    }
    let syn::Data::Enum(data) = &input.data else {
        return Ok(TokenStream::new());
    };
    let name = &input.ident;
    let unique = variant_id(&format!(
        "{} {:?}",
        input.to_token_stream(),
        input.ident.span()
    ));
    let helper = format_ident!("__errlanes_schema_{name}_{unique}");
    let exported = format_ident!("{name}Schema");
    let vis = &input.vis;
    let mut fields = TokenStream::new();
    let mut variants = data.variants.clone();
    for variant in &mut variants {
        let id = variant_id(&variant.ident.to_string());
        let mut origin = format!("{}::{}", helper, variant.ident);
        let mut delegate = false;
        let mut from = false;
        for attr in &variant.attrs {
            if attr.path().is_ident("rejection") {
                attr.parse_nested_meta(|m| {
                    if m.path.is_ident("origin") {
                        origin = m.value()?.parse::<syn::LitStr>()?.value();
                    } else if m.path.is_ident("delegate") {
                        delegate = true;
                    } else if m.path.is_ident("from") {
                        from = true;
                    } else if m.input.peek(Token![=]) {
                        let _: syn::Expr = m.value()?.parse()?;
                    }
                    Ok(())
                })?;
            }
        }
        // The `retain` below strips `#[rejection(..)]`, which would
        // silently drop a `delegate`/`from` variant's `source()` along with
        // it — mark the payload field directly first so the imported
        // variant keeps the same source after import.
        let payload_index = if delegate || from {
            Some(
                crate::classify::payload_field(&variant.fields, &variant.ident, "this variant")?
                    .index,
            )
        } else {
            None
        };
        variant
            .attrs
            .retain(|a| !a.path().is_ident("lift") && !a.path().is_ident("rejection"));
        variant
            .attrs
            .push(parse_quote!(#[rejection(origin = #origin)]));
        for (index, field) in variant.fields.iter_mut().enumerate() {
            let ty = &field.ty;
            fields.extend(quote! {
                impl errlanes::RejectionField<#id, #index> for #name { type Type = #ty; }
            });
            if payload_index == Some(index)
                && !field.attrs.iter().any(|a| a.path().is_ident("source"))
            {
                field.attrs.push(parse_quote!(#[source]));
            }
            // The caller-supplied source type is hygienic across renamed dependencies.
            field.ty = syn::parse2(
                quote!(<__ErrlanesSource as errlanes::RejectionField<#id, #index>>::Type),
            )?;
        }
    }
    let schema = quote!(#variants).to_string();
    // The source is a macro metavariable, substituted into schema by the callback.
    Ok(quote! {
        #fields
        #[doc(hidden)]
        #[macro_export]
        macro_rules! #helper {
            ($callback:path, [$($context:tt)*], $source:ty) => {
                $callback! { [$($context)*] [$source] #schema }
            };
        }
        #[doc(hidden)]
        #vis use #helper as #exported;
    })
}

fn is_placeholder(variant: &syn::Variant) -> bool {
    variant.attrs.iter().any(|a| a.path().is_ident("compose"))
}

pub fn expand(mut item: ItemEnum) -> syn::Result<TokenStream> {
    for variant in &item.variants {
        let mut flatten = false;
        for attr in &variant.attrs {
            if attr.path().is_ident("flatten") {
                return Err(syn::Error::new_spanned(
                    attr,
                    "use #[compose(flatten)] and the placeholder name as prefix; use explicit lifts for custom names",
                ));
            }
            if attr.path().is_ident("compose") {
                let option = attr.parse_args::<syn::Ident>().map_err(|_| {
                    syn::Error::new_spanned(
                        attr,
                        "expected #[compose(flatten)]; the placeholder name supplies the prefix, with no prefix or rename arguments",
                    )
                })?;
                if option != "flatten" || flatten {
                    return Err(syn::Error::new_spanned(
                        attr,
                        "expected exactly one #[compose(flatten)] on a source placeholder",
                    ));
                }
                flatten = true;
            }
        }
    }
    let Some(index) = item.variants.iter().position(is_placeholder) else {
        // The attribute owns these derives, including redundant user requests.
        // Keep all unrelated derives and add each of ours exactly once.
        let mut attrs = Vec::new();
        for attr in item.attrs {
            if attr.path().is_ident("derive") {
                let derives = attr.parse_args_with(
                    syn::punctuated::Punctuated::<syn::Path, Token![,]>::parse_terminated,
                )?;
                let derives: Vec<_> = derives
                    .into_iter()
                    .filter(|path| {
                        !path.segments.last().is_some_and(|segment| {
                            segment.ident == "Rejection" || segment.ident == "Lift"
                        })
                    })
                    .collect();
                if !derives.is_empty() {
                    attrs.push(parse_quote!(#[derive(#(#derives),*)]));
                }
            } else {
                attrs.push(attr);
            }
        }
        item.attrs = attrs;
        item.attrs.insert(
            0,
            parse_quote!(#[derive(errlanes::Rejection, errlanes::Lift)]),
        );
        return Ok(quote!(#item));
    };
    let variant = &item.variants[index];
    let syn::Fields::Unnamed(fields) = &variant.fields else {
        return Err(syn::Error::new_spanned(
            variant,
            "compose(flatten) requires one source family: Name(Source)",
        ));
    };
    if fields.unnamed.len() != 1 {
        return Err(syn::Error::new_spanned(
            fields,
            "compose(flatten) requires one source family: Name(Source)",
        ));
    }
    let source = &fields.unnamed[0].ty;
    let syn::Type::Path(source_path) = source else {
        return Err(syn::Error::new_spanned(
            source,
            "compose(flatten) requires a named source enum",
        ));
    };
    if source_path.qself.is_some() {
        return Err(syn::Error::new_spanned(
            source,
            "compose(flatten) requires a named source enum, not an associated type",
        ));
    }
    for attr in &item.attrs {
        if attr.path().is_ident("lift") {
            let registration = attr.parse_args::<crate::lift::Registration>()?;
            if registration.source.to_token_stream().to_string()
                == source.to_token_stream().to_string()
            {
                return Err(syn::Error::new_spanned(
                    variant,
                    "source family already mapped; choose whole-family #[compose(flatten)] or explicit #[lift] mappings for that source",
                ));
            }
        }
    }
    let mut helper = source_path.path.clone();
    let last = helper.segments.last_mut().unwrap();
    if !matches!(last.arguments, syn::PathArguments::None) {
        return Err(syn::Error::new_spanned(
            source,
            "generic rejection composition is unsupported; use a concrete enum",
        ));
    }
    last.ident = format_ident!("{}Schema", last.ident);
    Ok(quote! { #helper!(errlanes::__compose_rejection, [#item], #source); })
}

struct Callback {
    item: ItemEnum,
    source: syn::Type,
    schema: syn::LitStr,
}
impl Parse for Callback {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let item;
        syn::bracketed!(item in input);
        let item = item.parse()?;
        let source;
        syn::bracketed!(source in input);
        Ok(Self {
            item,
            source: source.parse()?,
            schema: input.parse()?,
        })
    }
}
pub fn callback(input: TokenStream) -> syn::Result<TokenStream> {
    let Callback {
        mut item,
        source,
        schema,
    } = syn::parse2(input)?;
    let index = item.variants.iter().position(is_placeholder).unwrap();
    let placeholder = &item.variants[index];
    let prefix = placeholder.ident.to_string();
    fn substitute(tokens: TokenStream, source: &syn::Type) -> TokenStream {
        use proc_macro2::{Group, TokenTree};
        tokens
            .into_iter()
            .flat_map(|token| match token {
                TokenTree::Ident(name) if name == "__ErrlanesSource" => source.to_token_stream(),
                TokenTree::Group(group) => {
                    let mut result =
                        Group::new(group.delimiter(), substitute(group.stream(), source));
                    result.set_span(group.span());
                    TokenStream::from(TokenTree::Group(result))
                }
                other => TokenStream::from(other),
            })
            .collect()
    }
    let variants = substitute(schema.value().parse()?, &source);
    let parsed: ItemEnum = syn::parse2(quote!(enum Schema { #variants }))?;
    let mut names: std::collections::HashSet<String> = item
        .variants
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != index)
        .map(|(_, v)| v.ident.to_string())
        .collect();
    fn origin(variant: &syn::Variant) -> syn::Result<Option<String>> {
        let mut origin = None;
        for attr in &variant.attrs {
            if attr.path().is_ident("rejection") {
                attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("origin") {
                        origin = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                    } else if meta.input.peek(Token![=]) {
                        let _: syn::Expr = meta.value()?.parse()?;
                    }
                    Ok(())
                })?;
            }
        }
        Ok(origin)
    }
    let mut origins = std::collections::HashSet::new();
    for variant in &item.variants {
        if let Some(origin) = origin(variant)? {
            origins.insert(origin);
        }
    }
    let mut imported = Vec::new();
    for mut variant in parsed.variants {
        if let Some(origin) = origin(&variant)?
            && !origins.insert(origin)
        {
            return Err(syn::Error::new_spanned(
                placeholder,
                "the same rejection leaf arrives through multiple imports; narrow the source families or map explicitly to a canonical destination",
            ));
        }
        let original = variant.ident.clone();
        variant.ident = format_ident!("{prefix}{original}");
        if !names.insert(variant.ident.to_string()) {
            return Err(syn::Error::new_spanned(
                &variant.ident,
                "composed variant name collision; change the placeholder name or use explicit lifts",
            ));
        }
        variant
            .attrs
            .push(parse_quote!(#[lift(#source::#original)]));
        variant
            .attrs
            .push(parse_quote!(#[rejection(forward = #source::#original)]));
        imported.push(variant);
    }
    let mut variants = syn::punctuated::Punctuated::new();
    for (i, variant) in item.variants.into_iter().enumerate() {
        if i == index {
            variants.extend(imported.clone());
        } else {
            variants.push(variant);
        }
    }
    item.variants = variants;
    item.attrs.push(parse_quote!(#[lift(#source, strict)]));
    expand(item)
}
