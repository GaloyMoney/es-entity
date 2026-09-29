use convert_case::{Case, Casing};
use darling::{FromDeriveInput, FromVariant, ast};
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
    repo: Option<Path>,
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
    constraint: Option<Path>,
    #[darling(default)]
    with: Option<Path>,
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

    if input.repo.is_none() {
        for v in variants {
            if v.constraint.is_some() {
                return Err(darling::Error::custom(
                    "#[rejection(constraint = ...)] requires enum-level #[rejection(repo = ...)]",
                )
                .with_span(&v.ident));
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

    if let Some(repo) = &input.repo {
        let mut lift_arms = Vec::new();
        for v in variants {
            let Some(constraint) = &v.constraint else {
                continue;
            };
            let variant_ident = &v.ident;
            match (&v.with, v.fields.style) {
                (Some(with), _) => {
                    lift_arms.push(quote! {
                        if constraint == Some(#constraint) {
                            return Ok((#with)(cv));
                        }
                    });
                }
                (None, ast::Style::Unit) => {
                    lift_arms.push(quote! {
                        if constraint == Some(#constraint) {
                            return Ok(Self::#variant_ident);
                        }
                    });
                }
                (None, _) => {
                    return Err(darling::Error::custom(
                        "a `constraint = ...` variant with fields needs `with = path::to::fn`",
                    )
                    .with_span(&v.ident));
                }
            }
        }

        tokens.extend(quote! {
            impl errlanes::LiftConstraint<#repo> for #ident {
                fn lift(cv: #repo) -> Result<Self, errlanes::Fatal> {
                    let constraint = errlanes::HasConstraint::constraint(&cv);
                    #(#lift_arms)*
                    Err(errlanes::Fatal::invariant(format!(
                        "unmapped constraint {}",
                        errlanes::HasConstraint::constraint_name(&cv).unwrap_or("?")
                    )))
                }
            }
        });
    }

    Ok(tokens)
}
