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
        for attr in &variant.attrs {
            if attr.path().is_ident("rejection") {
                attr.parse_nested_meta(|m| {
                    if m.path.is_ident("origin") {
                        origin = m.value()?.parse::<syn::LitStr>()?.value();
                    } else if m.input.peek(Token![=]) {
                        let _: syn::Expr = m.value()?.parse()?;
                    }
                    Ok(())
                })?;
            }
        }
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
            // The caller-supplied source type is hygienic across renamed dependencies.
            field.ty = syn::parse2(
                quote!(<__ErrlanesSource as errlanes::RejectionField<#id, #index>>::Type),
            )?;
            for attr in &mut field.attrs {
                if attr.path().is_ident("from") {
                    *attr = parse_quote!(#[source]);
                }
            }
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

pub fn expand(mut item: ItemEnum) -> syn::Result<TokenStream> {
    let Some(index) = item
        .variants
        .iter()
        .position(|v| v.attrs.iter().any(|a| a.path().is_ident("flatten")))
    else {
        item.attrs
            .insert(0, parse_quote!(#[derive(errlanes::Rejection)]));
        return Ok(quote!(#item));
    };
    let variant = &item.variants[index];
    let syn::Fields::Unnamed(fields) = &variant.fields else {
        return Err(syn::Error::new_spanned(
            variant,
            "flatten requires one source family: Name(Source)",
        ));
    };
    if fields.unnamed.len() != 1 {
        return Err(syn::Error::new_spanned(
            fields,
            "flatten requires one source family",
        ));
    }
    let source = &fields.unnamed[0].ty;
    let syn::Type::Path(source_path) = source else {
        return Err(syn::Error::new_spanned(
            source,
            "flatten requires a named source enum",
        ));
    };
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
    let index = item
        .variants
        .iter()
        .position(|v| v.attrs.iter().any(|a| a.path().is_ident("flatten")))
        .unwrap();
    let placeholder = &item.variants[index];
    let mut prefix = String::new();
    let mut renames = std::collections::HashMap::<String, syn::Ident>::new();
    for attr in &placeholder.attrs {
        if attr.path().is_ident("flatten") {
            if matches!(attr.meta, syn::Meta::Path(_)) {
                continue;
            }
            attr.parse_nested_meta(|m| {
                if m.path.is_ident("prefix") {
                    prefix = m.value()?.parse::<syn::LitStr>()?.value();
                } else if m.path.is_ident("rename") {
                    m.parse_nested_meta(|r| {
                        let from = r
                            .path
                            .get_ident()
                            .ok_or_else(|| r.error("expected variant name"))?
                            .to_string();
                        let to: syn::Ident = r.value()?.parse()?;
                        renames.insert(from, to);
                        Ok(())
                    })?;
                } else {
                    return Err(
                        m.error("expected prefix or rename(SourceVariant = DestinationVariant)")
                    );
                }
                Ok(())
            })?;
        }
    }
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
        variant.ident = renames
            .remove(&original.to_string())
            .unwrap_or_else(|| format_ident!("{prefix}{original}"));
        if !names.insert(variant.ident.to_string()) {
            return Err(syn::Error::new_spanned(
                &variant.ident,
                "flattened variant name collision; use prefix or rename",
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
    if !renames.is_empty() {
        return Err(syn::Error::new_spanned(
            placeholder,
            "rename names an unknown source variant",
        ));
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
