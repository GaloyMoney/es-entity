use darling::{FromDeriveInput, FromVariant, ast, util::Flag};
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Field, Ident};

#[derive(Debug, FromDeriveInput)]
#[darling(attributes(lane), supports(enum_any))]
struct ClassifyInput {
    ident: Ident,
    data: ast::Data<ClassifyVariant, ()>,
}

#[derive(Debug, FromVariant)]
#[darling(attributes(lane))]
struct ClassifyVariant {
    ident: Ident,
    fields: ast::Fields<Field>,
    #[darling(default)]
    rejected: Flag,
    #[darling(default)]
    denied: Flag,
    #[darling(default)]
    transient: Flag,
    #[darling(default)]
    fatal: Flag,
    #[darling(default)]
    delegate: Flag,
    /// `#[lane(external)]`: a variant wrapping a foreign, non-lanes-aware
    /// error, classified via `lane_of` at dispatch time. `dyn` is a
    /// reserved keyword and cannot appear bare inside `#[lane(...)]`.
    #[darling(default)]
    external: Flag,
    #[darling(default)]
    sqlx: Flag,
}

pub fn derive(ast: &syn::DeriveInput) -> darling::Result<TokenStream> {
    let input = ClassifyInput::from_derive_input(ast)?;
    let ident = &input.ident;
    let variants = match &input.data {
        ast::Data::Enum(v) => v,
        ast::Data::Struct(_) => {
            return Err(
                darling::Error::custom("Classify can only be derived for enums")
                    .with_span(&input.ident),
            );
        }
    };

    let mut arms = Vec::new();
    for v in variants {
        let variant_ident = &v.ident;
        let selected = [
            v.rejected.is_present(),
            v.denied.is_present(),
            v.transient.is_present(),
            v.fatal.is_present(),
            v.delegate.is_present(),
            v.external.is_present(),
            v.sqlx.is_present(),
        ]
        .into_iter()
        .filter(|x| *x)
        .count();

        if selected != 1 {
            return Err(darling::Error::custom(
                "every variant needs exactly one #[lane(rejected|denied|transient|fatal|delegate|external|sqlx)]",
            )
            .with_span(&v.ident));
        }

        let needs_inner = v.delegate.is_present() || v.external.is_present() || v.sqlx.is_present();
        if needs_inner && v.fields.style != ast::Style::Tuple {
            return Err(darling::Error::custom(
                "#[lane(delegate|external|sqlx)] requires a single-field tuple variant",
            )
            .with_span(&v.ident));
        }

        let pat = match v.fields.style {
            ast::Style::Unit => quote! { Self::#variant_ident },
            ast::Style::Tuple => quote! { Self::#variant_ident(__inner) },
            ast::Style::Struct => quote! { Self::#variant_ident { .. } },
        };

        let arm = if v.rejected.is_present() {
            quote! { #pat => errlanes::Lane::Rejected }
        } else if v.denied.is_present() {
            quote! { #pat => errlanes::Lane::Denied }
        } else if v.transient.is_present() {
            quote! { #pat => errlanes::Lane::Transient }
        } else if v.fatal.is_present() {
            quote! { #pat => errlanes::Lane::Fatal }
        } else if v.delegate.is_present() {
            quote! { #pat => errlanes::Classify::lane(__inner) }
        } else if v.external.is_present() {
            quote! { #pat => errlanes::lane_of(__inner).unwrap_or(errlanes::Lane::Fatal) }
        } else {
            quote! { #pat => errlanes::sqlx::lane_of_sqlx(__inner) }
        };
        arms.push(arm);
    }

    Ok(quote! {
        impl errlanes::Classify for #ident {
            fn lane(&self) -> errlanes::Lane {
                match self {
                    #(#arms),*
                }
            }
        }
    })
}
