use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use syn::{Fields, Path, Token, parse::Parse, spanned::Spanned};

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
pub(crate) struct Case {
    path: Path,
    inner: Option<Box<Case>>,
}
impl Case {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn leaf_path(&self) -> &Path {
        match &self.inner {
            Some(inner) => inner.leaf_path(),
            None => &self.path,
        }
    }

    fn is_nested(&self) -> bool {
        self.inner.is_some()
    }

    fn key(&self) -> String {
        let path = self.path.to_token_stream().to_string();
        match &self.inner {
            Some(inner) => format!("{path}({})", inner.key()),
            None => path,
        }
    }

    fn ancestor_keys(&self) -> Vec<String> {
        let mut paths = Vec::new();
        let mut level = self;
        loop {
            paths.push(level.path.to_token_stream().to_string());
            match &level.inner {
                Some(inner) => level = inner,
                None => break,
            }
        }
        (1..paths.len())
            .map(|depth| {
                paths[..depth - 1]
                    .iter()
                    .rev()
                    .fold(paths[depth - 1].clone(), |key, path| {
                        format!("{path}({key})")
                    })
            })
            .collect()
    }

    fn nest(&self, leaf: &dyn Fn(&Path) -> TokenStream) -> TokenStream {
        match &self.inner {
            Some(inner) => {
                let path = &self.path;
                let inner = inner.nest(leaf);
                quote! { #path(#inner) }
            }
            None => leaf(&self.path),
        }
    }
}
impl Parse for Case {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let path = input.parse()?;
        let inner = if input.peek(syn::token::Paren) {
            let content;
            let paren = syn::parenthesized!(content in input);
            let inner: Case = content.parse()?;
            if !content.is_empty() {
                return Err(syn::Error::new(
                    paren.span.join(),
                    "a nested lift case takes exactly one inner case",
                ));
            }
            Some(Box::new(inner))
        } else {
            None
        };
        Ok(Self { path, inner })
    }
}
pub(crate) struct Mapping {
    pub case: Case,
    pub with: Option<Path>,
    pub field: Option<syn::Ident>,
    pub into: bool,
    pub when: Option<Path>,
}
impl Parse for Mapping {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let case = input.parse()?;
        let mut with = None;
        let mut field = None;
        let mut into = false;
        let mut when = None;
        while input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            let key: syn::Ident = input.parse()?;
            if key == "when" {
                if when.is_some() {
                    return Err(syn::Error::new_spanned(key, "duplicate `when`"));
                }
                input.parse::<Token![=]>()?;
                when = Some(input.parse()?);
                continue;
            }
            if key == "into" {
                if into {
                    return Err(syn::Error::new_spanned(key, "duplicate `into`"));
                }
                if with.is_some() || field.is_some() {
                    return Err(syn::Error::new_spanned(
                        key,
                        "`into` cannot be combined with `field` or `with`",
                    ));
                }
                into = true;
                continue;
            }
            if into && (key == "field" || key == "with") {
                return Err(syn::Error::new_spanned(
                    key,
                    "`into` cannot be combined with `field` or `with`",
                ));
            }
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
                    "expected `with = mapper`, `field = name`, `into`, or `when = predicate`",
                ));
            }
        }
        Ok(Self {
            case,
            with,
            field,
            into,
            when,
        })
    }
}
pub fn derive(input: &syn::DeriveInput) -> syn::Result<TokenStream> {
    if let syn::Data::Struct(data) = &input.data {
        return derive_struct(input, data);
    }
    let syn::Data::Enum(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            input,
            "Lift can only be derived for enums or structs",
        ));
    };
    let mut registrations = Vec::new();
    for attr in &input.attrs {
        if attr.path().is_ident("lift") {
            registrations.push(attr.parse_args::<Registration>()?);
        }
    }
    let mut mappings = Vec::new();
    let mut unguarded: Vec<String> = Vec::new();
    for variant in &data.variants {
        for field in &variant.fields {
            if let Some(attr) = field.attrs.iter().find(|a| a.path().is_ident("lift")) {
                return Err(syn::Error::new_spanned(
                    attr,
                    "field lift mappings are only supported on struct destinations",
                ));
            }
        }
        for attr in &variant.attrs {
            if !attr.path().is_ident("lift") {
                continue;
            }
            let mapping = attr.parse_args::<Mapping>()?;
            let key = mapping.case.key();
            let must_follow = |unguarded_key: &str| {
                syn::Error::new_spanned(
                    attr,
                    format!(
                        "unguarded arm for `{}` must come after its guarded arms",
                        unguarded_key.replace(' ', "")
                    ),
                )
            };
            if unguarded.contains(&key) {
                return Err(if mapping.when.is_some() {
                    must_follow(&key)
                } else {
                    syn::Error::new_spanned(attr, "source variant mapped more than once")
                });
            }
            if let Some(ancestor) = mapping
                .case
                .ancestor_keys()
                .into_iter()
                .find(|ancestor| unguarded.contains(ancestor))
            {
                return Err(must_follow(&ancestor));
            }
            if mapping.when.is_none() {
                unguarded.push(key);
            }
            // `#[lift(Payload)]` (the registered source itself, no variant
            // suffix) is a whole-value arm: `Payload` is a struct source,
            // and the entire value becomes the one field of the destination
            // variant. `#[lift(Source::Variant)]` is the per-variant
            // forwarding arm. A bare source matching a registration wins the
            // whole-value reading; otherwise the last segment is popped off
            // and matched as a variant, as before.
            let case_key = mapping.case.path().to_token_stream().to_string();
            let whole_value = registrations
                .iter()
                .any(|r| r.source.to_token_stream().to_string() == case_key);
            if whole_value && mapping.case.is_nested() {
                return Err(syn::Error::new_spanned(
                    attr,
                    "a whole-value lift cannot be nested",
                ));
            }
            let owner = if whole_value {
                mapping.case.path().clone()
            } else {
                let mut owner = mapping.case.path().clone();
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
            mappings.push((owner, whole_value, variant, mapping, attr));
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
        for (owner, _, _, mapping, attr) in &mappings {
            if owner.to_token_stream().to_string() != key {
                continue;
            }
            if mapping.case.is_nested() && !registration.partial {
                return Err(syn::Error::new_spanned(
                    attr,
                    format!(
                        "a nested lift case leaves `{}` partially mapped; register \
                         `#[lift({}, unhandled = fatal)]`",
                        mapping
                            .case
                            .path()
                            .to_token_stream()
                            .to_string()
                            .replace(' ', ""),
                        key.replace(' ', ""),
                    ),
                ));
            }
            if let Some(when) = &mapping.when {
                let case_key = mapping.case.key();
                let caught = mappings.iter().any(|(_, _, _, other, _)| {
                    other.when.is_none() && other.case.key() == case_key
                });
                if !registration.partial && !caught {
                    return Err(syn::Error::new_spanned(
                        when,
                        format!(
                            "guards leave `{0}` partially mapped; add an unguarded arm for it \
                             or register `#[lift({1}, unhandled = fatal)]`",
                            case_key.replace(' ', ""),
                            key.replace(' ', ""),
                        ),
                    ));
                }
            }
        }
        for (owner, whole_value, variant, mapping, _) in &mappings {
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
            let guard = mapping.when.as_ref().map(|pred| quote!(if #pred(&payload)));
            let bound = case.nest(&|leaf| quote!(#leaf(payload)));
            let arm = if let Some(mapper) = &mapping.with {
                // The mapper consumes the whole selected source; supports genuine shape changes.
                let whole = case.nest(&|leaf| quote!(#leaf { .. }));
                match &mapping.when {
                    Some(pred) if *whole_value => {
                        quote! { value @ #whole if #pred(&value) => Ok(#mapper(value)) }
                    }
                    Some(pred) => {
                        quote! { #bound if #pred(&payload) => Ok(#mapper(#bound)) }
                    }
                    None => quote! { value @ #whole => Ok(#mapper(value)) },
                }
            } else if mapping.into {
                if *whole_value {
                    return Err(syn::Error::new_spanned(
                        case.path(),
                        "an `into` lift requires a source variant (`Source::Variant`)",
                    ));
                }
                // Only the payload is converted. The tuple pattern makes rustc
                // check that the source also has exactly one unnamed field.
                let converted = quote!(::core::convert::Into::into(payload));
                match &variant.fields {
                    Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                        quote! { #bound #guard => Ok(Self::#dest(#converted)) }
                    }
                    Fields::Named(fields) if fields.named.len() == 1 => {
                        let field = &fields.named[0].ident;
                        quote! { #bound #guard => Ok(Self::#dest { #field: #converted }) }
                    }
                    _ => {
                        return Err(syn::Error::new_spanned(
                            &variant.ident,
                            "an `into` lift needs exactly one destination field",
                        ));
                    }
                }
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
                        quote! { #bound #guard => Ok(Self::#dest(payload.#field)) }
                    }
                    Fields::Named(fields) if fields.named.len() == 1 => {
                        let dest_field = &fields.named[0].ident;
                        quote! {
                            #bound #guard => Ok(Self::#dest { #dest_field: payload.#field })
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
                let case = case.path();
                let guard = mapping.when.as_ref().map(|pred| quote!(if #pred(&value)));
                match &variant.fields {
                    Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                        quote! { value @ #case { .. } #guard => Ok(Self::#dest(value)) }
                    }
                    Fields::Named(fields) if fields.named.len() == 1 => {
                        let field = &fields.named[0].ident;
                        quote! { value @ #case { .. } #guard => Ok(Self::#dest { #field: value }) }
                    }
                    _ => {
                        return Err(syn::Error::new_spanned(
                            &variant.ident,
                            "a whole-value lift (`#[lift(Source)]` with no variant) needs \
                             exactly one destination field to hold the source value",
                        ));
                    }
                }
            } else if mapping.when.is_some() {
                match &variant.fields {
                    Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                        quote! { #bound #guard => Ok(Self::#dest(payload)) }
                    }
                    Fields::Named(fields) if fields.named.len() == 1 => {
                        let field = &fields.named[0].ident;
                        quote! { #bound #guard => Ok(Self::#dest { #field: payload }) }
                    }
                    _ => {
                        return Err(syn::Error::new_spanned(
                            &variant.ident,
                            "a guarded plain arm needs exactly one destination field",
                        ));
                    }
                }
            } else {
                match &variant.fields {
                    Fields::Unit => {
                        let pattern = case.nest(&|leaf| quote!(#leaf));
                        quote! { #pattern => Ok(Self::#dest) }
                    }
                    Fields::Unnamed(fields) => {
                        let args: Vec<_> = (0..fields.unnamed.len())
                            .map(|i| format_ident!("field_{i}"))
                            .collect();
                        let pattern = case.nest(&|leaf| quote!(#leaf(#(#args),*)));
                        quote! { #pattern => Ok(Self::#dest(#(#args),*)) }
                    }
                    Fields::Named(fields) => {
                        let args: Vec<_> = fields.named.iter().map(|f| &f.ident).collect();
                        let pattern = case.nest(&|leaf| quote!(#leaf { #(#args),* }));
                        quote! { #pattern => Ok(Self::#dest { #(#args),* }) }
                    }
                }
            };
            arms.push(quote! { #(#cfg)* #arm });
        }
        out.extend(conversion_impl(input, &source, registration.partial, &arms));
    }
    Ok(out)
}

fn conversion_impl(
    input: &syn::DeriveInput,
    source: &Path,
    partial: bool,
    arms: &[TokenStream],
) -> TokenStream {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    // Strict mode emits only `From`: errlanes' blanket `impl<X, P: From<X>>
    // Lift<X> for P` supplies the `Lift` view with `Unmapped = Infallible`,
    // so one call-site method (`lift`) covers strict and partial alike.
    // Emitting both here would collide with that blanket (E0119).
    if partial {
        let unhandled = syn::Ident::new("unhandled", source.span());
        quote! {
            impl #impl_generics errlanes::Lift<#source> for #name #ty_generics #where_clause {
                type Unmapped = #source;
                fn lift(source: #source) -> Result<Self, Self::Unmapped> {
                    match source { #(#arms,)* #unhandled => Err(#unhandled) }
                }
            }
        }
    } else if arms.is_empty() {
        // A source without members: nothing to map, and the empty match
        // diverges, so the code after it would be unreachable.
        quote! {
            impl #impl_generics From<#source> for #name #ty_generics #where_clause {
                fn from(source: #source) -> Self {
                    match source {}
                }
            }
        }
    } else {
        quote! {
            impl #impl_generics From<#source> for #name #ty_generics #where_clause {
                fn from(source: #source) -> Self {
                    let mapped: Result<Self, core::convert::Infallible> =
                        match source { #(#arms,)* };
                    match mapped {
                        Ok(mapped) => mapped, Err(never) => match never {},
                    }
                }
            }
        }
    }
}

/// A struct maps exactly one named-field source variant into its own fields.
/// Keep this grammar separate from enum registrations: `variant` is meaningful
/// only on a struct destination, and enum mappings retain their existing syntax.
struct StructRegistration {
    source: Path,
    variant: syn::Ident,
    partial: bool,
}

impl Parse for StructRegistration {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let source: Path = input.parse()?;
        let mut variant = None;
        let mut partial = None;
        while input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }
            let key: syn::Ident = input.parse()?;
            match key.to_string().as_str() {
                "variant" => {
                    if variant.is_some() {
                        return Err(syn::Error::new_spanned(key, "duplicate `variant`"));
                    }
                    input.parse::<Token![=]>()?;
                    variant = Some(input.parse()?);
                }
                "strict" | "unhandled" => {
                    if partial.is_some() {
                        return Err(syn::Error::new_spanned(
                            key,
                            "choose either `strict` or `unhandled = fatal` once",
                        ));
                    }
                    partial = Some(key == "unhandled");
                    if key == "unhandled" {
                        input.parse::<Token![=]>()?;
                        let fatal: syn::Ident = input.parse()?;
                        if fatal != "fatal" {
                            return Err(syn::Error::new_spanned(fatal, "expected fatal"));
                        }
                    }
                }
                _ => {
                    return Err(syn::Error::new_spanned(
                        key,
                        "expected `variant = Case`, `strict`, or `unhandled = fatal`",
                    ));
                }
            }
        }
        let variant = variant.ok_or_else(|| {
            syn::Error::new_spanned(&source, "a struct lift requires `variant = Case`")
        })?;
        Ok(Self {
            source,
            variant,
            partial: partial.unwrap_or(false),
        })
    }
}

struct SourceField(syn::Ident);

impl Parse for SourceField {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let key: syn::Ident = input.parse()?;
        if key != "from" {
            return Err(syn::Error::new_spanned(
                key,
                "expected `from = source_field`",
            ));
        }
        input.parse::<Token![=]>()?;
        let field = input.parse()?;
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        }
        Ok(Self(field))
    }
}

fn derive_struct(input: &syn::DeriveInput, data: &syn::DataStruct) -> syn::Result<TokenStream> {
    if let Fields::Unnamed(fields) = &data.fields {
        return derive_newtype(input, fields);
    }
    if matches!(data.fields, Fields::Unit) {
        return derive_unit(input);
    }
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            input,
            "a struct lift requires named fields or a single-field tuple struct",
        ));
    };
    let mut attrs = input.attrs.iter().filter(|a| a.path().is_ident("lift"));
    let attr = attrs.next().ok_or_else(|| {
        syn::Error::new_spanned(
            &input.ident,
            "a struct lift requires #[lift(Source, variant = Case)]",
        )
    })?;
    if let Some(extra) = attrs.next() {
        return Err(syn::Error::new_spanned(
            extra,
            "a struct lift supports exactly one source registration",
        ));
    }
    let StructRegistration {
        source,
        variant,
        partial,
    } = attr.parse_args()?;
    let mut case = source.clone();
    // Infer the enum's arguments from the match scrutinee. Explicit lifetime
    // arguments are not permitted on variant patterns.
    case.segments.last_mut().expect("source path").arguments = syn::PathArguments::None;
    case.segments.push(variant.into());
    let mut bindings = Vec::new();
    let mut assignments = Vec::new();
    let mut sources = std::collections::HashSet::new();
    for (i, field) in fields.named.iter().enumerate() {
        let dest = field.ident.as_ref().expect("named field");
        let mut attrs = field.attrs.iter().filter(|a| a.path().is_ident("lift"));
        let from = match attrs.next() {
            Some(attr) => attr.parse_args::<SourceField>()?.0,
            None => dest.clone(),
        };
        if let Some(extra) = attrs.next() {
            return Err(syn::Error::new_spanned(
                extra,
                "duplicate field lift mapping",
            ));
        }
        if !sources.insert(from.to_string().trim_start_matches("r#").to_owned()) {
            return Err(syn::Error::new_spanned(
                from,
                "source field mapped more than once",
            ));
        }
        let binding = format_ident!("__lift_field_{i}");
        bindings.push(quote! { #from: #binding });
        assignments.push(quote! { #dest: ::core::convert::Into::into(#binding) });
    }
    // No `..`: rustc checks the complete source payload, including missing
    // fields and shape/type mismatches. No fields are silently discarded.
    let arm = quote! { #case { #(#bindings),* } => Ok(Self { #(#assignments),* }) };
    Ok(conversion_impl(input, &source, partial, &[arm]))
}

/// A unit struct lifts a whole source value and drops it. Only the source
/// path is accepted: a case of an enum source is routed by the family's arm.
struct UnitRegistration {
    source: Path,
}

impl Parse for UnitRegistration {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let source: Path = input.parse()?;
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            if let Some(key) = input.parse::<Option<syn::Ident>>()? {
                return Err(syn::Error::new_spanned(
                    &key,
                    if key == "variant" {
                        "a unit struct lifts a whole value; a case of an enum source is routed by the family's arm, `with` if it has no payload"
                    } else {
                        "a unit struct lift takes only the source path"
                    },
                ));
            }
        }
        Ok(Self { source })
    }
}

fn derive_unit(input: &syn::DeriveInput) -> syn::Result<TokenStream> {
    let mut attrs = input.attrs.iter().filter(|a| a.path().is_ident("lift"));
    let attr = attrs.next().ok_or_else(|| {
        syn::Error::new_spanned(&input.ident, "a unit struct lift requires #[lift(Source)]")
    })?;
    if let Some(extra) = attrs.next() {
        return Err(syn::Error::new_spanned(
            extra,
            "a struct lift supports exactly one source registration",
        ));
    }
    let UnitRegistration { source } = attr.parse_args()?;
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics From<#source> for #name #ty_generics #where_clause {
            fn from(_: #source) -> Self {
                Self
            }
        }
    })
}

/// A newtype either wraps the whole source or explicitly projects one field
/// of a source struct. Unlike the named-field enum-variant mapping, the
/// projection intentionally drops the other fields.
struct NewtypeRegistration {
    source: Path,
    field: Option<syn::Member>,
}

impl Parse for NewtypeRegistration {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let source: Path = input.parse()?;
        let mut field = None;
        while input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }
            let key: syn::Ident = input.parse()?;
            if key != "field" {
                return Err(syn::Error::new_spanned(
                    key,
                    "a tuple struct lift supports only `field = name`",
                ));
            }
            if field.is_some() {
                return Err(syn::Error::new_spanned(key, "duplicate `field`"));
            }
            input.parse::<Token![=]>()?;
            field = Some(input.parse()?);
        }
        Ok(Self { source, field })
    }
}

fn derive_newtype(
    input: &syn::DeriveInput,
    fields: &syn::FieldsUnnamed,
) -> syn::Result<TokenStream> {
    if fields.unnamed.len() != 1 {
        return Err(syn::Error::new_spanned(
            fields,
            "a tuple struct lift requires exactly one field",
        ));
    }
    for field in &fields.unnamed {
        if let Some(attr) = field.attrs.iter().find(|a| a.path().is_ident("lift")) {
            return Err(syn::Error::new_spanned(
                attr,
                "put the field projection on the struct: #[lift(Source, field = name)]",
            ));
        }
    }
    let mut attrs = input.attrs.iter().filter(|a| a.path().is_ident("lift"));
    let attr = attrs.next().ok_or_else(|| {
        syn::Error::new_spanned(
            &input.ident,
            "a tuple struct lift requires #[lift(Source)] or #[lift(Source, field = name)]",
        )
    })?;
    if let Some(extra) = attrs.next() {
        return Err(syn::Error::new_spanned(
            extra,
            "a struct lift supports exactly one source registration",
        ));
    }
    let NewtypeRegistration { source, field } = attr.parse_args()?;
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let value = match field {
        Some(field) => quote!(::core::convert::Into::into(source.#field)),
        None => quote!(source),
    };
    Ok(quote! {
        impl #impl_generics From<#source> for #name #ty_generics #where_clause {
            fn from(source: #source) -> Self {
                Self(#value)
            }
        }
    })
}
