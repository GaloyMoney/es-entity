use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use syn::{Fields, Path, Token, parse::Parse};

pub(crate) struct Registration {
    pub(crate) source: Path,
    partial: bool,
}
impl Parse for Registration {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let source = input.parse()?;
        let mut partial = false;
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            let mode: syn::Ident = input.parse()?;
            if mode == "unhandled" {
                input.parse::<Token![=]>()?;
                let fatal: syn::Ident = input.parse()?;
                if fatal != "fatal" {
                    return Err(syn::Error::new_spanned(fatal, "expected fatal"));
                }
                partial = true;
            } else if mode != "strict" {
                return Err(syn::Error::new_spanned(
                    mode,
                    "expected strict or unhandled = fatal",
                ));
            }
        }
        Ok(Self { source, partial })
    }
}
pub(crate) struct Mapping {
    pub case: Path,
    pub with: Option<Path>,
    pub field: Option<syn::Ident>,
}
impl Parse for Mapping {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let case = input.parse()?;
        let mut with = None;
        let mut field = None;
        while input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            let key: syn::Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            if key == "with" {
                if field.is_some() {
                    return Err(syn::Error::new_spanned(
                        key,
                        "`field` and `with` cannot be combined",
                    ));
                }
                with = Some(input.parse()?);
            } else if key == "field" {
                if with.is_some() {
                    return Err(syn::Error::new_spanned(
                        key,
                        "`field` and `with` cannot be combined",
                    ));
                }
                field = Some(input.parse()?);
            } else {
                return Err(syn::Error::new_spanned(
                    key,
                    "expected `with = mapper` or `field = name`",
                ));
            }
        }
        Ok(Self { case, with, field })
    }
}
pub fn derive(input: &syn::DeriveInput) -> syn::Result<TokenStream> {
    let syn::Data::Enum(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            input,
            "Lift can only be derived for enums",
        ));
    };
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let mut registrations = Vec::new();
    for attr in &input.attrs {
        if attr.path().is_ident("lift") {
            registrations.push(attr.parse_args::<Registration>()?);
        }
    }
    let mut mappings = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for variant in &data.variants {
        for attr in &variant.attrs {
            if !attr.path().is_ident("lift") {
                continue;
            }
            let mapping = attr.parse_args::<Mapping>()?;
            if !seen.insert(mapping.case.to_token_stream().to_string()) {
                return Err(syn::Error::new_spanned(
                    attr,
                    "source variant mapped more than once",
                ));
            }
            // `#[lift(Payload)]` (the registered source itself, no variant
            // suffix) is a whole-value arm: `Payload` is a struct source,
            // and the entire value becomes the one field of the destination
            // variant. `#[lift(Source::Variant)]` is the per-variant
            // forwarding arm. A bare source matching a registration wins the
            // whole-value reading; otherwise the last segment is popped off
            // and matched as a variant, as before.
            let case_key = mapping.case.to_token_stream().to_string();
            let whole_value = registrations
                .iter()
                .any(|r| r.source.to_token_stream().to_string() == case_key);
            let owner = if whole_value {
                mapping.case.clone()
            } else {
                let mut owner = mapping.case.clone();
                owner.segments.pop();
                owner.segments.pop_punct();
                owner
            };
            if !registrations.iter().any(|r| {
                r.source.to_token_stream().to_string() == owner.to_token_stream().to_string()
            }) {
                return Err(syn::Error::new_spanned(
                    attr,
                    "source family requires enum-level #[lift(Source)] registration",
                ));
            }
            mappings.push((owner, whole_value, variant, mapping));
        }
    }
    let mut out = TokenStream::new();
    let mut owners = std::collections::HashSet::new();
    for registration in registrations {
        let source = registration.source;
        let key = source.to_token_stream().to_string();
        if !owners.insert(key.clone()) {
            return Err(syn::Error::new_spanned(source, "duplicate lift source"));
        }
        let mut arms = Vec::new();
        for (owner, whole_value, variant, mapping) in &mappings {
            if owner.to_token_stream().to_string() != key {
                continue;
            }
            let case = &mapping.case;
            let dest = &variant.ident;
            let cfg: Vec<_> = variant
                .attrs
                .iter()
                .filter(|a| a.path().is_ident("cfg"))
                .collect();
            let arm = if let Some(mapper) = &mapping.with {
                // The mapper consumes the whole selected source; supports genuine shape changes.
                quote! { value @ #case { .. } => Ok(#mapper(value)) }
            } else if let Some(field) = &mapping.field {
                // A projection of one field out of the source variant's
                // payload. `#case(payload)` is a tuple-destructuring
                // pattern, not `#case { .. }`: the source variant must be a
                // single-field tuple variant, and a named-field or unit
                // source variant fails right here as an ordinary type
                // error — the derive cannot know a foreign variant's field
                // names, so it cannot validate the shape ahead of time.
                match &variant.fields {
                    Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                        quote! { #case(payload) => Ok(Self::#dest(payload.#field)) }
                    }
                    Fields::Named(fields) if fields.named.len() == 1 => {
                        let dest_field = &fields.named[0].ident;
                        quote! {
                            #case(payload) => Ok(Self::#dest { #dest_field: payload.#field })
                        }
                    }
                    _ => {
                        return Err(syn::Error::new_spanned(
                            &variant.ident,
                            "a `field` lift needs exactly one destination field",
                        ));
                    }
                }
            } else if *whole_value {
                // `Payload` is a struct source: the whole value becomes the
                // one field of the destination variant.
                match &variant.fields {
                    Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                        quote! { value @ #case { .. } => Ok(Self::#dest(value)) }
                    }
                    Fields::Named(fields) if fields.named.len() == 1 => {
                        let field = &fields.named[0].ident;
                        quote! { value @ #case { .. } => Ok(Self::#dest { #field: value }) }
                    }
                    _ => {
                        return Err(syn::Error::new_spanned(
                            &variant.ident,
                            "a whole-value lift (`#[lift(Source)]` with no variant) needs \
                             exactly one destination field to hold the source value",
                        ));
                    }
                }
            } else {
                match &variant.fields {
                    Fields::Unit => quote! { #case => Ok(Self::#dest) },
                    Fields::Unnamed(fields) => {
                        let args: Vec<_> = (0..fields.unnamed.len())
                            .map(|i| format_ident!("field_{i}"))
                            .collect();
                        quote! { #case(#(#args),*) => Ok(Self::#dest(#(#args),*)) }
                    }
                    Fields::Named(fields) => {
                        let args: Vec<_> = fields.named.iter().map(|f| &f.ident).collect();
                        quote! { #case { #(#args),* } => Ok(Self::#dest { #(#args),* }) }
                    }
                }
            };
            arms.push(quote! { #(#cfg)* #arm });
        }
        // Strict mode emits only `From`: errlanes' blanket `impl<X, P: From<X>>
        // Lift<X> for P` supplies the `Lift` view with `Unmapped = Infallible`,
        // so one call-site method (`widen`) covers strict and partial alike.
        // Emitting both here would collide with that blanket (E0119).
        if registration.partial {
            out.extend(quote! {
                impl #impl_generics errlanes::Lift<#source> for #name #ty_generics #where_clause {
                    type Unmapped = #source;
                    fn lift(source: #source) -> Result<Self, Self::Unmapped> {
                        match source { #(#arms,)* unhandled => Err(unhandled) }
                    }
                }
            });
        } else {
            out.extend(quote! {
                impl #impl_generics From<#source> for #name #ty_generics #where_clause {
                    fn from(source: #source) -> Self {
                        let mapped: Result<Self, core::convert::Infallible> =
                            match source { #(#arms,)* };
                        match mapped {
                            Ok(mapped) => mapped, Err(never) => match never {},
                        }
                    }
                }
            });
        }
    }
    Ok(out)
}
