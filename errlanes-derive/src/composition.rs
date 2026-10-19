mod imports;

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
        variant.attrs.push(if from {
            parse_quote!(#[rejection(origin = #origin, from)])
        } else {
            parse_quote!(#[rejection(origin = #origin)])
        });
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
            if from && payload_index == Some(index) {
                let generic = generic_destination(ty, name);
                fields.extend(quote! {
                    impl<__ErrlanesDestination>
                        errlanes::RejectionPayload<__ErrlanesDestination, #id, #index>
                        for #name
                    {
                        type Type = #generic;
                    }
                });
                field.ty = syn::parse2(quote!(
                    <__ErrlanesSource as errlanes::RejectionPayload<
                        __ErrlanesDestination, #id, #index
                    >>::Type
                ))?;
            } else {
                field.ty = syn::parse2(
                    quote!(<__ErrlanesSource as errlanes::RejectionField<#id, #index>>::Type),
                )?;
            }
        }
    }
    // Every source this family converts from (a strict `#[lift(X)]`, which
    // compose pushes for each import, or a transitive `via` record) is
    // exported under the hash of its path as written here, so a composing
    // family can name the type without resolving the path itself.
    let mut source_ids = Vec::new();
    for attr in &input.attrs {
        if !attr.path().is_ident("lift") {
            continue;
        }
        if crate::lift::is_via(attr)? {
            let crate::lift::Via { via, id } = attr.parse_args()?;
            fields.extend(quote! {
                impl<__ErrlanesDestination> errlanes::RejectionSource<__ErrlanesDestination, #id> for #name {
                    type Type = <#via as errlanes::RejectionSource<__ErrlanesDestination, #id>>::Type;
                }
            });
            source_ids.push(id);
        } else if let Some(registration) = crate::lift::registration(attr)?
            && !registration.partial.is_partial()
        {
            let id = variant_id(&imports::key(&registration.source));
            let source = &registration.source;
            fields.extend(quote! {
                impl<__ErrlanesDestination> errlanes::RejectionSource<__ErrlanesDestination, #id> for #name { type Type = #source; }
            });
            source_ids.push(id);
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
                $callback! { [$($context)*] [$source] #schema [#(#source_ids),*] }
            };
        }
        #[doc(hidden)]
        #vis use #helper as #exported;
    })
}

pub fn compose(args: TokenStream, item: ItemEnum) -> syn::Result<TokenStream> {
    if args.is_empty() {
        expand(item)
    } else {
        imports::start(args, item)
    }
}

/// Finalize a composition: every import has already been resolved into real
/// variants, so all that is left is to own the derive list.
pub fn expand(mut item: ItemEnum) -> syn::Result<TokenStream> {
    for variant in &item.variants {
        for attr in &variant.attrs {
            if attr.path().is_ident("flatten") {
                return Err(syn::Error::new_spanned(
                    attr,
                    "`flatten` is not a composition attribute; list the family as #[errlanes::compose(Source as Prefix)]",
                ));
            }
            if attr.path().is_ident("compose") {
                return Err(imports::unexpected(attr));
            }
        }
    }
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
    Ok(quote!(#item))
}

struct Callback {
    context: imports::Context,
    source: syn::Type,
    schema: syn::LitStr,
    ids: Vec<u64>,
}
impl Parse for Callback {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let context;
        syn::bracketed!(context in input);
        let source;
        syn::bracketed!(source in input);
        let schema = input.parse()?;
        let mut ids = Vec::new();
        if input.peek(syn::token::Bracket) {
            let list;
            syn::bracketed!(list in input);
            for id in list.parse_terminated(syn::LitInt::parse, Token![,])? {
                ids.push(id.base10_parse()?);
            }
        }
        Ok(Self {
            context: context.parse()?,
            source: source.parse()?,
            schema,
            ids,
        })
    }
}
pub fn callback(input: TokenStream) -> syn::Result<TokenStream> {
    let Callback {
        context,
        source,
        schema,
        ids,
    } = syn::parse2(input)?;
    fn substitute(
        tokens: TokenStream,
        source: &syn::Type,
        destination: &syn::Ident,
    ) -> TokenStream {
        use proc_macro2::{Group, TokenTree};
        tokens
            .into_iter()
            .flat_map(|token| match token {
                TokenTree::Ident(name) if name == "__ErrlanesSource" => source.to_token_stream(),
                TokenTree::Ident(name) if name == "__ErrlanesDestination" => {
                    destination.to_token_stream()
                }
                TokenTree::Group(group) => {
                    let mut result = Group::new(
                        group.delimiter(),
                        substitute(group.stream(), source, destination),
                    );
                    result.set_span(group.span());
                    TokenStream::from(TokenTree::Group(result))
                }
                other => TokenStream::from(other),
            })
            .collect()
    }
    let variants = substitute(schema.value().parse()?, &source, context.destination());
    let parsed: ItemEnum = syn::parse2(quote!(enum Schema { #variants }))?;
    context.receive(parsed.variants, ids)
}

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

/// A payload this family itself imported is a `RejectionPayload` projection
/// naming this family as its destination; re-exported, it must stay generic
/// over whichever family imports it next.
fn generic_destination(ty: &syn::Type, owner: &syn::Ident) -> syn::Type {
    let mut ty = ty.clone();
    if let syn::Type::Path(path) = &mut ty
        && let Some(qself) = &path.qself
        && let Some(segment) = path.path.segments.iter_mut().nth(qself.position - 1)
        && segment.ident == "RejectionPayload"
        && let syn::PathArguments::AngleBracketed(arguments) = &mut segment.arguments
        && let Some(syn::GenericArgument::Type(syn::Type::Path(destination))) =
            arguments.args.first_mut()
        && destination.path.is_ident(owner)
    {
        *destination = parse_quote!(__ErrlanesDestination);
    }
    ty
}
