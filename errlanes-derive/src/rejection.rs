use convert_case::{Case, Casing};
use darling::{FromDeriveInput, FromVariant, ast, util::PathList};
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Field, Ident, Path};

#[derive(Debug, FromDeriveInput)]
#[darling(attributes(rejection), supports(enum_any))]
struct RejectionInput {
    ident: Ident,
    data: ast::Data<RejectionVariant, ()>,
    #[darling(default)]
    code_prefix: Option<String>,
    #[darling(default)]
    lift: PathList,
}

#[derive(Debug, FromVariant)]
#[darling(attributes(rejection))]
struct RejectionVariant {
    ident: Ident,
    fields: ast::Fields<Field>,
    #[darling(default)]
    code: Option<String>,
    #[darling(default)]
    level: Option<String>,
    #[darling(default)]
    key: Option<Path>,
    /// Which `lift = X` target this `key` belongs to. A `key`'s own path
    /// (e.g. `UserConstraint::EmailKey`) names the *key* type, not the
    /// *lift* (`Liftable`) type — the two are unrelated types the macro has
    /// no way to connect syntactically — so with more than one `lift`
    /// target this is required to disambiguate. With exactly one target
    /// it's implied and may be omitted.
    #[darling(default)]
    via: Option<Path>,
    #[darling(default)]
    with: Option<Path>,
}

/// Ident-by-ident path equality — syn::Path has no `PartialEq`.
fn paths_equal(a: &Path, b: &Path) -> bool {
    a.segments.len() == b.segments.len()
        && a.segments
            .iter()
            .zip(b.segments.iter())
            .all(|(x, y)| x.ident == y.ident)
}

impl RejectionVariant {
    fn is_delegating(&self) -> bool {
        self.fields.style == ast::Style::Tuple
            && self.fields.fields.len() == 1
            && self.fields.fields[0]
                .attrs
                .iter()
                .any(|a| a.path().is_ident("from"))
    }

    fn delegate_ty(&self) -> Option<&syn::Type> {
        self.is_delegating().then(|| &self.fields.fields[0].ty)
    }

    fn pattern(&self, bind_inner: bool) -> TokenStream {
        let ident = &self.ident;
        match self.fields.style {
            ast::Style::Unit => quote! { Self::#ident },
            ast::Style::Tuple if bind_inner => quote! { Self::#ident(__inner) },
            ast::Style::Tuple => quote! { Self::#ident(..) },
            ast::Style::Struct => quote! { Self::#ident { .. } },
        }
    }

    fn level_expr(&self) -> TokenStream {
        match self.level.as_deref() {
            Some("trace") => quote! { errlanes::Level::Trace },
            Some("debug") => quote! { errlanes::Level::Debug },
            Some("info") | None => quote! { errlanes::Level::Info },
            Some("warn") => quote! { errlanes::Level::Warn },
            Some("error") => quote! { errlanes::Level::Error },
            Some(other) => {
                let msg = format!(
                    "unknown rejection level `{other}`, expected one of trace/debug/info/warn/error"
                );
                quote! { compile_error!(#msg) }
            }
        }
    }

    fn leaf_code(&self, prefix: &Option<String>) -> String {
        match &self.code {
            Some(c) => c.clone(),
            None => {
                let base = self.ident.to_string().to_case(Case::Constant);
                match prefix {
                    Some(p) => format!("{p}{base}"),
                    None => base,
                }
            }
        }
    }
}

pub fn derive(ast: &syn::DeriveInput) -> darling::Result<TokenStream> {
    let input = RejectionInput::from_derive_input(ast)?;
    let ident = &input.ident;
    let code_ident = quote::format_ident!("{}Code", ident);
    let variants = match &input.data {
        ast::Data::Enum(v) => v,
        ast::Data::Struct(_) => {
            return Err(
                darling::Error::custom("Rejection can only be derived for enums")
                    .with_span(&input.ident),
            );
        }
    };

    if input.lift.is_empty() {
        for v in variants {
            if v.key.is_some() {
                return Err(darling::Error::custom(
                    "#[rejection(key = ...)] requires enum-level #[rejection(lift = ...)]",
                )
                .with_span(&v.ident));
            }
        }
    } else {
        for v in variants {
            if v.key.is_none() {
                continue;
            }
            match (&v.via, input.lift.len()) {
                (Some(via), _) => {
                    if !input.lift.iter().any(|lift_ty| paths_equal(via, lift_ty)) {
                        return Err(darling::Error::custom(
                            "#[rejection(via = ...)] does not name a declared #[rejection(lift = ...)] target",
                        )
                        .with_span(&v.ident));
                    }
                }
                (None, 1) => {}
                (None, _) => {
                    return Err(darling::Error::custom(
                        "more than one #[rejection(lift = ...)] target: this `key = ...` variant needs `via = ...` to say which one it lifts from",
                    )
                    .with_span(&v.ident));
                }
            }
        }
    }

    let mut code_variants = Vec::new();
    let mut into_str_arms = Vec::new();
    let mut code_match_arms = Vec::new();
    let mut level_match_arms = Vec::new();
    let mut leaf_codes = Vec::new();

    for v in variants {
        let variant_ident = &v.ident;
        if let Some(inner_ty) = v.delegate_ty() {
            code_variants.push(quote! {
                #variant_ident(<#inner_ty as errlanes::Rejection>::Code)
            });
            into_str_arms.push(quote! {
                #code_ident::#variant_ident(inner) => inner.into()
            });
            let pat = v.pattern(true);
            code_match_arms.push(quote! {
                #pat => #code_ident::#variant_ident(<#inner_ty as errlanes::Rejection>::code(__inner))
            });
            level_match_arms.push(quote! {
                #pat => <#inner_ty as errlanes::Rejection>::level(__inner)
            });
        } else {
            code_variants.push(quote! { #variant_ident });
            let leaf = v.leaf_code(&input.code_prefix);
            into_str_arms.push(quote! {
                #code_ident::#variant_ident => #leaf
            });
            leaf_codes.push(leaf);
            let pat = v.pattern(false);
            code_match_arms.push(quote! {
                #pat => #code_ident::#variant_ident
            });
            let level = v.level_expr();
            level_match_arms.push(quote! {
                #pat => #level
            });
        }
    }

    let mut tokens = quote! {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum #code_ident {
            #(#code_variants),*
        }

        impl #code_ident {
            pub const ALL: &'static [&'static str] = &[#(#leaf_codes),*];
        }

        impl std::fmt::Display for #code_ident {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str((*self).into())
            }
        }

        impl From<#code_ident> for &'static str {
            fn from(code: #code_ident) -> &'static str {
                match code {
                    #(#into_str_arms),*
                }
            }
        }

        impl errlanes::Rejection for #ident {
            type Code = #code_ident;

            fn code(&self) -> Self::Code {
                match self {
                    #(#code_match_arms),*
                }
            }

            fn level(&self) -> errlanes::Level {
                match self {
                    #(#level_match_arms),*
                }
            }
        }
    };

    let single_target = input.lift.len() == 1;
    for lift_ty in input.lift.iter() {
        let mut lift_arms = Vec::new();
        for v in variants {
            let Some(key) = &v.key else {
                continue;
            };
            let belongs = match &v.via {
                Some(via) => paths_equal(via, lift_ty),
                None => single_target,
            };
            if !belongs {
                continue;
            }
            let variant_ident = &v.ident;
            match (&v.with, v.fields.style) {
                (Some(with), _) => {
                    lift_arms.push(quote! {
                        if key == Some(#key) {
                            return Ok((#with)(x));
                        }
                    });
                }
                (None, ast::Style::Unit) => {
                    lift_arms.push(quote! {
                        if key == Some(#key) {
                            return Ok(Self::#variant_ident);
                        }
                    });
                }
                (None, _) => {
                    return Err(darling::Error::custom(
                        "a `key = ...` variant with fields needs `with = path::to::fn`",
                    )
                    .with_span(&v.ident));
                }
            }
        }

        tokens.extend(quote! {
            impl errlanes::Lift<#lift_ty> for #ident {
                fn lift(x: #lift_ty) -> Result<Self, errlanes::Fatal> {
                    let key = errlanes::Liftable::key(&x);
                    #(#lift_arms)*
                    let unknown = match key {
                        Some(k) => <<#lift_ty as errlanes::Liftable>::Key as Into<&'static str>>::into(k).to_string(),
                        None => "unknown".to_string(),
                    };
                    Err(errlanes::Fatal::from_error(errlanes::FatalKind::Invariant, x)
                        .with_context(unknown))
                }
            }
        });
    }

    Ok(tokens)
}
