use darling::{FromDeriveInput, ast, util::PathList};
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Field, Ident, Type};

#[derive(Debug, FromDeriveInput)]
#[darling(attributes(failure), supports(struct_newtype))]
struct FailureInput {
    ident: Ident,
    data: ast::Data<(), Field>,
    #[darling(default)]
    from: PathList,
    #[darling(default)]
    lift: PathList,
    #[darling(default)]
    sqlx: darling::util::Flag,
}

fn rejection_ty(field_ty: &Type) -> darling::Result<&Type> {
    if let Type::Path(p) = field_ty
        && let Some(seg) = p.path.segments.last()
        && seg.ident == "Fail"
        && let syn::PathArguments::AngleBracketed(args) = &seg.arguments
        && let Some(syn::GenericArgument::Type(t)) = args.args.first()
    {
        return Ok(t);
    }
    Err(darling::Error::custom(
        "#[derive(errlanes::Failure)] requires a single-field newtype wrapping `errlanes::Fail<D>`",
    ))
}

pub fn derive(ast: &syn::DeriveInput) -> darling::Result<TokenStream> {
    let input = FailureInput::from_derive_input(ast)?;
    let ident = &input.ident;
    let fields = match &input.data {
        ast::Data::Struct(f) => f,
        ast::Data::Enum(_) => {
            return Err(
                darling::Error::custom("Failure can only be derived for a newtype struct")
                    .with_span(&input.ident),
            );
        }
    };
    let field = fields.fields.first().ok_or_else(|| {
        darling::Error::custom("Failure requires exactly one field").with_span(&input.ident)
    })?;
    let d_ty = rejection_ty(&field.ty)?;

    let mut tokens = quote! {
        impl std::fmt::Display for #ident {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                std::fmt::Display::fmt(&self.0, f)
            }
        }

        impl std::error::Error for #ident {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }

        impl errlanes::Failure for #ident {
            type Rejection = #d_ty;

            fn into_fail(self) -> errlanes::Fail<Self::Rejection> {
                self.0
            }

            fn from_fail(f: errlanes::Fail<Self::Rejection>) -> Self {
                Self(f)
            }

            fn as_fail(&self) -> &errlanes::Fail<Self::Rejection> {
                &self.0
            }
        }

        impl From<#d_ty> for #ident {
            fn from(d: #d_ty) -> Self {
                Self(errlanes::Fail::Rejected(d))
            }
        }

        impl From<errlanes::Fail<#d_ty>> for #ident {
            fn from(f: errlanes::Fail<#d_ty>) -> Self {
                Self(f)
            }
        }

        impl From<errlanes::Transient> for #ident {
            fn from(t: errlanes::Transient) -> Self {
                Self(errlanes::Fail::Transient(t))
            }
        }

        impl From<errlanes::Fatal> for #ident {
            fn from(x: errlanes::Fatal) -> Self {
                Self(errlanes::Fail::Fatal(x))
            }
        }

        impl From<errlanes::Denied> for #ident {
            fn from(d: errlanes::Denied) -> Self {
                Self(errlanes::Fail::Denied(d))
            }
        }

        impl From<errlanes::Exhausted> for #ident {
            fn from(e: errlanes::Exhausted) -> Self {
                Self(errlanes::Fail::from(e))
            }
        }

        impl From<errlanes::Fault> for #ident {
            fn from(f: errlanes::Fault) -> Self {
                Self(f.into())
            }
        }
    };

    if input.sqlx.is_present() {
        tokens.extend(quote! {
            impl From<sqlx::Error> for #ident {
                fn from(e: sqlx::Error) -> Self {
                    Self(errlanes::Fail::from(e))
                }
            }
        });
    }

    for upstream in input.from.iter() {
        tokens.extend(quote! {
            impl From<#upstream> for #ident {
                fn from(e: #upstream) -> Self {
                    Self(<#upstream as errlanes::Failure>::into_fail(e).widen())
                }
            }
        });
    }

    for lift in input.lift.iter() {
        tokens.extend(quote! {
            impl From<errlanes::Fail<#lift>> for #ident {
                fn from(f: errlanes::Fail<#lift>) -> Self {
                    Self(f.widen_with(<#d_ty as errlanes::Lift<#lift>>::lift))
                }
            }

            impl From<#lift> for #ident {
                fn from(x: #lift) -> Self {
                    errlanes::Fail::Rejected(x).into()
                }
            }
        });
    }

    Ok(tokens)
}
