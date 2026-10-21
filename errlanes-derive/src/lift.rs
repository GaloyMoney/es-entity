use proc_macro2::{Span, TokenStream};
use quote::{ToTokens, format_ident, quote};
use syn::{Fields, Path, Token, parse::Parse, spanned::Spanned};

pub(crate) struct Registration {
    pub(crate) source: Path,
    pub(crate) partial: bool,
}

/// `#[lift(via = Family, id = N)]`: composition's record that `Family`
/// converts from the source it exports under `N`, so the composing enum is
/// `From` that source too.
pub(crate) struct Via {
    pub(crate) via: Path,
    pub(crate) id: u64,
}
impl Parse for Via {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let key: syn::Ident = input.parse()?;
        if key != "via" {
            return Err(syn::Error::new_spanned(key, "expected `via = Family`"));
        }
        input.parse::<Token![=]>()?;
        let via = input.parse()?;
        input.parse::<Token![,]>()?;
        let key: syn::Ident = input.parse()?;
        if key != "id" {
            return Err(syn::Error::new_spanned(key, "expected `id = N`"));
        }
        input.parse::<Token![=]>()?;
        let id = input.parse::<syn::LitInt>()?.base10_parse()?;
        Ok(Self { via, id })
    }
}
impl ToTokens for Via {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let (via, id) = (&self.via, self.id);
        tokens.extend(quote!(via = #via, id = #id));
    }
}

pub(crate) fn is_via(attr: &syn::Attribute) -> syn::Result<bool> {
    attr.parse_args_with(|input: syn::parse::ParseStream| {
        let via = input.peek(syn::Ident) && input.peek2(Token![=]);
        input.parse::<TokenStream>()?;
        Ok(via)
    })
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
    braces: Option<Braces>,
}

/// The braces after a leaf: `{ .. }`, or field names optionally followed by
/// `..`.
struct Braces {
    bound: Vec<syn::Ident>,
    rest: bool,
}
impl Braces {
    const SHAPE: &'static str =
        "a leaf pattern takes `..`, or field names optionally followed by `..`";

    fn parse(content: syn::parse::ParseStream, span: Span) -> syn::Result<Self> {
        let mut bound: Vec<syn::Ident> = Vec::new();
        let mut rest = false;
        while !content.is_empty() {
            if content.peek(Token![..]) {
                content.parse::<Token![..]>()?;
                rest = true;
                if !content.is_empty() {
                    return Err(syn::Error::new(span, "`..` goes last in a leaf pattern"));
                }
                break;
            }
            let name: syn::Ident = content
                .parse()
                .map_err(|_| syn::Error::new(span, Self::SHAPE))?;
            if content.peek(Token![:]) {
                return Err(syn::Error::new_spanned(
                    name,
                    "a leaf pattern binds field names; it does not rename them",
                ));
            }
            if bound.contains(&name) {
                return Err(syn::Error::new_spanned(
                    &name,
                    format!("`{name}` is bound twice"),
                ));
            }
            bound.push(name);
            if content.is_empty() {
                break;
            }
            content.parse::<Token![,]>()?;
        }
        if bound.is_empty() && !rest {
            return Err(syn::Error::new(span, Self::SHAPE));
        }
        Ok(Self { bound, rest })
    }

    fn pattern(&self) -> TokenStream {
        let bound = &self.bound;
        let rest = self.rest.then(|| quote!(..));
        quote!({ #(#bound,)* #rest })
    }
}
impl Case {
    fn leaf_braces(&self) -> Option<&Braces> {
        match &self.inner {
            Some(inner) => inner.leaf_braces(),
            None => self.braces.as_ref(),
        }
    }

    fn leaf_bound(&self) -> &[syn::Ident] {
        self.leaf_braces().map_or(&[], |braces| &braces.bound)
    }

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
        let braces = if input.peek(syn::token::Brace) {
            let content;
            let brace = syn::braced!(content in input);
            let braces = Braces::parse(&content, brace.span.join())?;
            if input.peek(syn::token::Paren) {
                return Err(syn::Error::new(
                    brace.span.join(),
                    "`{ .. }` goes on the leaf",
                ));
            }
            Some(braces)
        } else {
            None
        };
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
        Ok(Self {
            path,
            inner,
            braces,
        })
    }
}
pub(crate) struct Mapping {
    pub case: Case,
    pub with: Option<Path>,
    pub field: Option<syn::Ident>,
    pub into: bool,
    pub when: Option<Path>,
    pub to: Option<syn::Ident>,
}

const ARM_KEYS: [&str; 5] = ["with", "field", "into", "when", "to"];

/// An enum-level `#[lift(..)]` is an arm, not a source registration, when a
/// mapping key follows its case.
pub(crate) fn is_arm(attr: &syn::Attribute) -> syn::Result<bool> {
    attr.parse_args_with(|input: syn::parse::ParseStream| {
        let fork = input.fork();
        let arm = fork.parse::<Case>().is_ok()
            && fork.parse::<Token![,]>().is_ok()
            && fork
                .parse::<syn::Ident>()
                .is_ok_and(|key| ARM_KEYS.iter().any(|arm| key == arm));
        input.parse::<TokenStream>()?;
        Ok(arm)
    })
}

/// The source registration an enum-level `#[lift(..)]` makes, or `None` when
/// it is an arm.
pub(crate) fn registration(attr: &syn::Attribute) -> syn::Result<Option<Registration>> {
    if is_arm(attr)? || is_via(attr)? {
        return Ok(None);
    }
    attr.parse_args().map(Some)
}

impl Parse for Mapping {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let case = input.parse()?;
        let mut mapping = Self {
            case,
            with: None,
            field: None,
            into: false,
            when: None,
            to: None,
        };
        let mut shape: Option<String> = None;
        while input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            let key: syn::Ident = input.parse()?;
            let name = key.to_string();
            if name == "to" {
                if mapping.to.is_some() {
                    return Err(syn::Error::new_spanned(key, "duplicate `to`"));
                }
                input.parse::<Token![=]>()?;
                mapping.to = Some(input.parse()?);
                continue;
            }
            if name == "when" {
                if mapping.when.is_some() {
                    return Err(syn::Error::new_spanned(key, "duplicate `when`"));
                }
                input.parse::<Token![=]>()?;
                mapping.when = Some(input.parse()?);
                continue;
            }
            if !matches!(name.as_str(), "with" | "field" | "into") {
                return Err(syn::Error::new_spanned(
                    key,
                    "expected `with = mapper`, `field = name`, `into`, \
                     `when = predicate`, or `to = Variant`",
                ));
            }
            if let Some(previous) = &shape {
                return Err(syn::Error::new_spanned(
                    key,
                    if *previous == name {
                        format!("duplicate `{name}`")
                    } else {
                        format!("`{name}` cannot be combined with `{previous}`")
                    },
                ));
            }
            match name.as_str() {
                "into" => mapping.into = true,
                _ => {
                    input.parse::<Token![=]>()?;
                    match name.as_str() {
                        "with" => mapping.with = Some(input.parse()?),
                        _ => mapping.field = Some(input.parse()?),
                    }
                }
            }
            shape = Some(name);
        }
        Ok(mapping)
    }
}
struct Arm<'a> {
    owner: Path,
    whole_value: bool,
    dest: Option<&'a syn::Variant>,
    mapping: Mapping,
    attr: &'a syn::Attribute,
}

fn admit(
    registrations: &[Registration],
    unguarded: &mut Vec<String>,
    attr: &syn::Attribute,
    mapping: &Mapping,
    enum_level: bool,
) -> syn::Result<(Path, bool)> {
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
    // suffix) is a whole-value arm: the entire value becomes the one field
    // of the destination variant. `#[lift(Source::Variant)]` is the
    // per-variant forwarding arm. A bare source matching a registration wins
    // the whole-value reading; otherwise the last segment is popped off and
    // matched as a variant, as before. An enum-level arm always names a case.
    let case_key = mapping.case.path().to_token_stream().to_string();
    let whole_value = !enum_level
        && registrations
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
    if !registrations
        .iter()
        .any(|r| r.source.to_token_stream().to_string() == owner.to_token_stream().to_string())
    {
        return Err(syn::Error::new_spanned(
            attr,
            "source family requires enum-level #[lift(Source)] registration",
        ));
    }
    Ok((owner, whole_value))
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
    let mut enum_arms = Vec::new();
    let mut vias = Vec::new();
    for attr in &input.attrs {
        if !attr.path().is_ident("lift") {
            continue;
        }
        if is_via(attr)? {
            vias.push(attr.parse_args::<Via>()?);
            continue;
        }
        match registration(attr)? {
            Some(registration) => registrations.push(registration),
            None => enum_arms.push(attr),
        }
    }
    for registration in &registrations {
        let mut owner = registration.source.clone();
        owner.segments.pop();
        owner.segments.pop_punct();
        let owner = owner.to_token_stream().to_string();
        if registrations
            .iter()
            .any(|other| other.source.to_token_stream().to_string() == owner)
        {
            return Err(syn::Error::new_spanned(
                &registration.source,
                "an enum-level arm needs `to = Variant`, `into` (`Self: From<Payload>`) or \
                 `with` (`fn(Source) -> Self`)",
            ));
        }
    }
    let mut arms: Vec<Arm> = Vec::new();
    let mut unguarded: Vec<String> = Vec::new();
    for attr in enum_arms {
        let mapping = attr.parse_args::<Mapping>()?;
        let dest = match &mapping.to {
            Some(to) => Some(
                data.variants
                    .iter()
                    .find(|variant| variant.ident == *to)
                    .ok_or_else(|| {
                        syn::Error::new_spanned(to, format!("no variant `{to}` to lift into"))
                    })?,
            ),
            None => {
                if mapping.field.is_some() {
                    return Err(syn::Error::new_spanned(
                        attr,
                        "an enum-level arm without `to` forwards the whole source case; \
                         use `into` or `with`",
                    ));
                }
                if !mapping.into && mapping.with.is_none() {
                    return Err(syn::Error::new_spanned(
                        attr,
                        "an enum-level arm needs `to = Variant`, `into` (`Self: From<Payload>`) \
                         or `with` (`fn(Source) -> Self`)",
                    ));
                }
                None
            }
        };
        let (owner, whole_value) = admit(&registrations, &mut unguarded, attr, &mapping, true)?;
        arms.push(Arm {
            owner,
            whole_value,
            dest,
            mapping,
            attr,
        });
    }
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
            if let Some(to) = &mapping.to {
                return Err(syn::Error::new_spanned(
                    to,
                    "`to` belongs on an enum-level arm; a variant arm already names its destination",
                ));
            }
            let (owner, whole_value) =
                admit(&registrations, &mut unguarded, attr, &mapping, false)?;
            arms.push(Arm {
                owner,
                whole_value,
                dest: Some(variant),
                mapping,
                attr,
            });
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
        let own: Vec<_> = arms
            .iter()
            .filter(|arm| arm.owner.to_token_stream().to_string() == key)
            .collect();
        for arm in &own {
            let Arm { mapping, attr, .. } = arm;
            binds_nothing_conflict(mapping)?;
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
                let caught = arms.iter().any(|other| {
                    other.mapping.when.is_none() && other.mapping.case.key() == case_key
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
        // An unguarded whole-value `into` accepts every value, so it is the
        // entire conversion: any other arm or catch-all after it is dead.
        if let Some(whole) = own
            .iter()
            .find(|arm| arm.whole_value && arm.mapping.when.is_none() && arm.mapping.into)
            && (registration.partial
                || own.iter().any(|arm| {
                    let guarded_whole = arm.whole_value && arm.mapping.when.is_some();
                    !(guarded_whole || std::ptr::eq(*arm, *whole))
                }))
        {
            return Err(syn::Error::new_spanned(
                whole.attr,
                "a whole-value `into` over an enum source is the entire conversion; \
                 it cannot be combined with other arms or `unhandled = fatal`",
            ));
        }
        let mut tokens = Vec::new();
        for arm in own {
            let (cfg, arm_tokens) = match arm.dest {
                Some(variant) => (
                    variant
                        .attrs
                        .iter()
                        .filter(|a| a.path().is_ident("cfg"))
                        .collect(),
                    variant_arm(&arm.mapping, arm.whole_value, variant)?,
                ),
                None => (Vec::new(), forwarding_arm(&arm.mapping)),
            };
            tokens.push(quote! { #(#cfg)* #arm_tokens });
        }
        out.extend(conversion_impl(
            input,
            &source,
            registration.partial,
            &tokens,
        ));
    }
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    for Via { via, id } in vias {
        out.extend(quote! {
            impl #impl_generics
                From<<#via as errlanes::RejectionSource<#name #ty_generics, #id>>::Type>
                for #name #ty_generics #where_clause
            {
                fn from(
                    source: <#via as errlanes::RejectionSource<#name #ty_generics, #id>>::Type,
                ) -> Self {
                    <Self as From<#via>>::from(<#via as From<
                        <#via as errlanes::RejectionSource<#name #ty_generics, #id>>::Type,
                    >>::from(source))
                }
            }
        });
    }
    Ok(out)
}

/// The whole source case becomes `Self`: `Self: From<Payload>` for `into`, or
/// a `fn(Source) -> Self` for `with`.
fn forwarding_arm(mapping: &Mapping) -> TokenStream {
    let case = &mapping.case;
    let payload_guard = mapping.when.as_ref().map(|pred| quote!(if #pred(&payload)));
    let bound = case.nest(&|leaf| quote!(#leaf(payload)));
    if let Some(mapper) = &mapping.with {
        let whole = case.nest(&|leaf| quote!(#leaf { .. }));
        return match &mapping.when {
            Some(pred) => quote! { #bound if #pred(&payload) => Ok(#mapper(#bound)) },
            None => quote! { value @ #whole => Ok(#mapper(value)) },
        };
    }
    quote! { #bound #payload_guard => Ok(::core::convert::Into::into(payload)) }
}

/// `Self::Dest(value)` or `Self::Dest { field: value }` for a destination
/// variant that holds exactly one field.
fn single_field(
    variant: &syn::Variant,
    value: TokenStream,
    what: &str,
) -> syn::Result<TokenStream> {
    let dest = &variant.ident;
    match &variant.fields {
        Fields::Unnamed(fields) if fields.unnamed.len() == 1 => Ok(quote! { Self::#dest(#value) }),
        Fields::Named(fields) if fields.named.len() == 1 => {
            let field = &fields.named[0].ident;
            Ok(quote! { Self::#dest { #field: #value } })
        }
        _ => Err(syn::Error::new_spanned(
            &variant.ident,
            format!("{what} needs exactly one destination field"),
        )),
    }
}

fn binds_nothing_conflict(mapping: &Mapping) -> syn::Result<()> {
    if mapping.case.leaf_braces().is_none() {
        return Ok(());
    }
    let binds_fields = !mapping.case.leaf_bound().is_empty();
    let message = if binds_fields
        && (mapping.with.is_some()
            || mapping.into
            || mapping.field.is_some()
            || mapping.when.is_some())
    {
        "a leaf that binds fields builds the destination; \
         `with`/`into`/`field`/`when` are not combined with it"
    } else if binds_fields {
        return Ok(());
    } else if mapping.with.is_some() {
        "`{ .. }` is the derived form of a `with` hook that drops the payload; \
         use one or the other"
    } else if mapping.into || mapping.field.is_some() || mapping.when.is_some() {
        "`{ .. }` binds nothing; `into`/`field`/`when` need the payload"
    } else {
        return Ok(());
    };
    Err(syn::Error::new_spanned(mapping.case.leaf_path(), message))
}

/// `Leaf { a: Into::into(a), .. }` for each bound field.
fn assignments(bound: &[syn::Ident]) -> Vec<TokenStream> {
    bound
        .iter()
        .map(|name| quote!(#name: ::core::convert::Into::into(#name)))
        .collect()
}

/// A `Source::Variant { .. }` arm drops the payload: the destination is a unit
/// variant, or a variant holding one unit leaf named by its field type. A
/// `Source::Variant { a, .. }` arm builds the destination from the named
/// fields.
fn braced_arm(
    mapping: &Mapping,
    whole_value: bool,
    variant: &syn::Variant,
) -> syn::Result<TokenStream> {
    let case = &mapping.case;
    let dest = &variant.ident;
    if whole_value {
        return Err(syn::Error::new_spanned(
            case.leaf_path(),
            "a whole-value lift has no case to bind; a leaf pattern goes on a source variant",
        ));
    }
    let braces = case.leaf_braces().expect("braced leaf");
    let leaf_pattern = braces.pattern();
    let pattern = case.nest(&|leaf| quote!(#leaf #leaf_pattern));
    let bound = &braces.bound;
    if bound.is_empty() {
        let built = match &variant.fields {
            Fields::Unit => quote!(Self::#dest),
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                let leaf = &fields.unnamed[0].ty;
                quote!(Self::#dest(#leaf))
            }
            Fields::Named(fields) if fields.named.len() == 1 => {
                let field = &fields.named[0].ident;
                let leaf = &fields.named[0].ty;
                quote!(Self::#dest { #field: #leaf })
            }
            _ => {
                return Err(syn::Error::new_spanned(
                    &variant.ident,
                    "a `{ .. }` arm binds nothing: the destination is a unit variant or \
                     holds one unit leaf",
                ));
            }
        };
        return Ok(quote! { #pattern => Ok(#built) });
    }
    let built = match &variant.fields {
        Fields::Named(fields)
            if fields.named.len() == bound.len()
                && fields
                    .named
                    .iter()
                    .all(|field| bound.iter().any(|name| Some(name) == field.ident.as_ref())) =>
        {
            let direct = fields.named.iter().filter_map(|field| field.ident.as_ref());
            let direct = direct.map(|name| quote!(#name: ::core::convert::Into::into(#name)));
            quote!(Self::#dest { #(#direct),* })
        }
        Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
            let leaf = &fields.unnamed[0].ty;
            let fields = assignments(bound);
            quote!(Self::#dest(#leaf { #(#fields),* }))
        }
        Fields::Named(fields) if fields.named.len() == 1 => {
            let field = &fields.named[0].ident;
            let leaf = &fields.named[0].ty;
            let fields = assignments(bound);
            quote!(Self::#dest { #field: #leaf { #(#fields),* } })
        }
        Fields::Unit => {
            return Err(syn::Error::new_spanned(
                &variant.ident,
                "a leaf that binds fields builds the destination; a unit variant has \
                 nowhere to put them",
            ));
        }
        _ => {
            return Err(syn::Error::new_spanned(
                &variant.ident,
                "a leaf that binds fields needs a destination with exactly those named \
                 fields, or one field whose type is built from them",
            ));
        }
    };
    Ok(quote! { #pattern => Ok(#built) })
}

fn variant_arm(
    mapping: &Mapping,
    whole_value: bool,
    variant: &syn::Variant,
) -> syn::Result<TokenStream> {
    let case = &mapping.case;
    let dest = &variant.ident;
    if case.leaf_braces().is_some() {
        return braced_arm(mapping, whole_value, variant);
    }
    let guard = mapping.when.as_ref().map(|pred| quote!(if #pred(&payload)));
    let bound = case.nest(&|leaf| quote!(#leaf(payload)));
    if let Some(mapper) = &mapping.with {
        // The mapper consumes the whole selected source; supports genuine shape changes.
        let whole = case.nest(&|leaf| quote!(#leaf { .. }));
        return Ok(match &mapping.when {
            Some(pred) if whole_value => {
                quote! { value @ #whole if #pred(&value) => Ok(#mapper(value)) }
            }
            Some(pred) => {
                quote! { #bound if #pred(&payload) => Ok(#mapper(#bound)) }
            }
            None => quote! { value @ #whole => Ok(#mapper(value)) },
        });
    }
    if mapping.into {
        if whole_value {
            // The binding takes the place of the source pattern, so the
            // source may be a struct or an enum.
            let guard = mapping.when.as_ref().map(|pred| quote!(if #pred(&value)));
            let converted = single_field(
                variant,
                quote!(::core::convert::Into::into(value)),
                "an `into` lift",
            )?;
            return Ok(quote! { value #guard => Ok(#converted) });
        }
        // Only the payload is converted. The tuple pattern makes rustc
        // check that the source also has exactly one unnamed field.
        let converted = single_field(
            variant,
            quote!(::core::convert::Into::into(payload)),
            "an `into` lift",
        )?;
        return Ok(quote! { #bound #guard => Ok(#converted) });
    }
    if let Some(member) = &mapping.field {
        if whole_value {
            return Err(syn::Error::new_spanned(
                case.path(),
                "a `field` lift requires a source variant (`Source::Variant`)",
            ));
        }
        // A projection of one field out of the source variant's
        // payload. `#case(payload)` is a tuple-destructuring
        // pattern, not `#case { .. }`: the source variant must be a
        // single-field tuple variant, and a named-field or unit
        // source variant fails right here as an ordinary type
        // error — the derive cannot know a foreign variant's field
        // names, so it cannot validate the shape ahead of time.
        let projected = single_field(variant, quote!(payload.#member), "a `field` lift")?;
        return Ok(quote! { #bound #guard => Ok(#projected) });
    }
    if whole_value {
        // `Payload` is a struct source: the whole value becomes the
        // one field of the destination variant.
        let case = case.path();
        let guard = mapping.when.as_ref().map(|pred| quote!(if #pred(&value)));
        let held = single_field(
            variant,
            quote!(value),
            "a whole-value lift (`#[lift(Source)]` with no variant)",
        )
        .map_err(|_| {
            syn::Error::new_spanned(
                &variant.ident,
                "a whole-value lift (`#[lift(Source)]` with no variant) needs \
                 exactly one destination field to hold the source value",
            )
        })?;
        return Ok(quote! { value @ #case { .. } #guard => Ok(#held) });
    }
    if mapping.when.is_some() {
        let kept = single_field(variant, quote!(payload), "a guarded plain arm")?;
        return Ok(quote! { #bound #guard => Ok(#kept) });
    }
    Ok(match &variant.fields {
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
    })
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
                        "a unit struct lifts a whole value; a case of an enum source is routed by the family's arm: write `S::V { .. }` on the arm to drop its payload"
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
