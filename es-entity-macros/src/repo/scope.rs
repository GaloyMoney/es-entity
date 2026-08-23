use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::*;

#[cfg(test)]
use convert_case::{Case, Casing};
#[cfg(test)]
use proc_macro2::Span;

/// One scope column: its enum variant ident (UpperCamel of the column name,
/// e.g. `partner_id` -> `PartnerId`), the column name, and its Rust type.
#[derive(Clone)]
pub struct ScopeCol<'a> {
    pub variant: syn::Ident,
    pub column_name: &'a syn::Ident,
    pub column_ty: &'a syn::Type,
}

impl ScopeCol<'_> {
    /// The SQL conjunct for this column's arm at the given parameter index.
    pub fn predicate(&self, param_idx: u32) -> String {
        format!("{} = ${}", self.column_name, param_idx)
    }

    /// The query binding for this column's arm (pairs with the dispatch
    /// arm's `__scope_val` pattern binding).
    pub fn arg_tokens(&self) -> TokenStream {
        let column_ty = self.column_ty;
        quote! { __scope_val as &#column_ty, }
    }
}

/// Test-only helper mirroring [`super::options::Column::scope_variant`]'s
/// default (no override) computation, for building [`ScopeCol`] fixtures
/// directly without going through a parsed `Column`.
#[cfg(test)]
fn variant_ident(column_name: &syn::Ident) -> syn::Ident {
    syn::Ident::new(
        &column_name.to_string().to_case(Case::UpperCamel),
        Span::call_site(),
    )
}

/// Info about a repo's scope columns, shared by the read-fn emitters.
///
/// A repo with one or more columns marked `scope` generates every read fn
/// (`find_by_*`, `find_all`, `list_by_*`, `list_for_*`, `list_for_filters*`)
/// with a leading `scope: impl Into<{Entity}Scope>` argument. At runtime the
/// fn dispatches on the scope: `All` executes the exact same SQL as an
/// unscoped repo, each scope column's variant executes a variant with that
/// column's additional `scope_column = $n` conjunct — all static, sargable
/// `es_query!` literals.
#[derive(Clone)]
pub struct ScopeInfo<'a> {
    /// The generated scope enum ident: `{Entity}Scope`.
    pub scope_ty: syn::Ident,
    /// The scope columns in declaration order (= variant order = dispatch
    /// arm order).
    pub cols: Vec<ScopeCol<'a>>,
}

impl<'a> ScopeInfo<'a> {
    pub fn from_opts(opts: &'a RepositoryOptions) -> Option<Self> {
        let cols: Vec<_> = opts
            .columns
            .scope_columns()
            .into_iter()
            .map(|col| ScopeCol {
                variant: col.scope_variant(),
                column_name: col.name(),
                column_ty: col.ty(),
            })
            .collect();
        if cols.is_empty() {
            return None;
        }
        Some(ScopeInfo {
            scope_ty: opts.scope_type_ident(),
            cols,
        })
    }

    /// The `scope: impl Into<{Entity}Scope>,` fn argument.
    pub fn fn_arg(&self) -> TokenStream {
        let scope_ty = &self.scope_ty;
        quote! { scope: impl Into<#scope_ty>, }
    }

    /// Forwarding token for the standalone -> `_in_op` delegation.
    pub fn fn_pass(&self) -> TokenStream {
        quote! { scope, }
    }

    /// Converts the `impl Into<_>` argument once at fn entry.
    pub fn convert(&self) -> TokenStream {
        quote! { let __scope = scope.into(); }
    }

    /// Runtime dispatch between the unscoped (`All`) query variant and one
    /// variant per scope column. `scoped_arm` is invoked once per column to
    /// build that column's arm body; the arm pattern binds `__scope_val: &T`.
    pub fn dispatch(
        &self,
        all_arm: TokenStream,
        scoped_arm: impl Fn(&ScopeCol<'a>) -> TokenStream,
    ) -> TokenStream {
        let scope_ty = &self.scope_ty;
        let arms = self.cols.iter().map(|col| {
            let variant = &col.variant;
            let body = scoped_arm(col);
            quote! { #scope_ty::#variant(__scope_val) => #body, }
        });
        quote! {
            match &__scope {
                #scope_ty::All => #all_arm,
                #(#arms)*
            }
        }
    }
}

/// Generates the per-repo scope enum:
///
/// ```ignore
/// pub enum UserScope {
///     All,
///     PartnerId(PartnerId),
///     CustomerId(CustomerId),
/// }
/// ```
///
/// plus `From<T>` / `From<&T>` conversions into each column's variant so call
/// sites can pass a scope value directly. Deliberately **no**
/// `From<Option<T>>`: mapping `None` to `All` would turn a stray `None` into
/// silent all-scope access.
pub struct ScopeType<'a> {
    entity: &'a syn::Ident,
    info: ScopeInfo<'a>,
}

impl<'a> ScopeType<'a> {
    pub fn new(opts: &'a RepositoryOptions) -> Option<Self> {
        ScopeInfo::from_opts(opts).map(|info| Self {
            entity: opts.entity(),
            info,
        })
    }
}

impl ToTokens for ScopeType<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let scope_ty = &self.info.scope_ty;
        let doc = format!(
            "Scope argument for [`{}`] repository reads: each column variant filters every \
             query by that scope column, `All` reads across all scopes (audited escape hatch).",
            self.entity,
        );

        let variants = self.info.cols.iter().map(|col| {
            let variant = &col.variant;
            let column_ty = col.column_ty;
            let vdoc = format!(
                "Restricts every read to rows whose `{}` column equals the value.",
                col.column_name,
            );
            quote! {
                #[doc = #vdoc]
                #variant(#column_ty),
            }
        });

        let from_impls = self.info.cols.iter().map(|col| {
            let variant = &col.variant;
            let column_ty = col.column_ty;
            quote! {
                impl From<#column_ty> for #scope_ty {
                    fn from(value: #column_ty) -> Self {
                        #scope_ty::#variant(value)
                    }
                }

                impl From<&#column_ty> for #scope_ty {
                    fn from(value: &#column_ty) -> Self {
                        #scope_ty::#variant(*value)
                    }
                }
            }
        });

        tokens.append_all(quote! {
            #[doc = #doc]
            #[derive(Debug, Clone, Copy)]
            pub enum #scope_ty {
                /// No scope filter — reads across all scopes.
                All,
                #(#variants)*
            }

            #(#from_impls)*

            impl From<&#scope_ty> for #scope_ty {
                fn from(value: &#scope_ty) -> Self {
                    *value
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_info<'a>(cols: &[(&'a syn::Ident, &'a syn::Type)]) -> (ScopeInfo<'a>, syn::Ident) {
        let entity = syn::Ident::new("Entity", Span::call_site());
        (
            ScopeInfo {
                scope_ty: syn::Ident::new("EntityScope", Span::call_site()),
                cols: cols
                    .iter()
                    .map(|(name, ty)| ScopeCol {
                        variant: variant_ident(name),
                        column_name: name,
                        column_ty: ty,
                    })
                    .collect(),
            },
            entity,
        )
    }

    #[test]
    fn scope_type_tokens_single() {
        let column_name = syn::Ident::new("partner_id", Span::call_site());
        let column_ty: syn::Type = syn::parse_str("PartnerId").unwrap();
        let (info, entity) = test_info(&[(&column_name, &column_ty)]);
        let scope_type = ScopeType {
            entity: &entity,
            info,
        };

        let mut tokens = TokenStream::new();
        scope_type.to_tokens(&mut tokens);
        let token_str = tokens.to_string();

        assert!(token_str.contains("pub enum EntityScope"));
        assert!(token_str.contains("PartnerId (PartnerId)"));
        assert!(!token_str.contains("Only"));
        assert!(token_str.contains("impl From < PartnerId > for EntityScope"));
        assert!(token_str.contains("impl From < & PartnerId > for EntityScope"));
        assert!(!token_str.contains("Option"));
    }

    #[test]
    fn scope_type_tokens_multi() {
        let partner = syn::Ident::new("partner_id", Span::call_site());
        let partner_ty: syn::Type = syn::parse_str("PartnerId").unwrap();
        let customer = syn::Ident::new("customer_id", Span::call_site());
        let customer_ty: syn::Type = syn::parse_str("CustomerId").unwrap();
        let (info, entity) = test_info(&[(&partner, &partner_ty), (&customer, &customer_ty)]);
        let scope_type = ScopeType {
            entity: &entity,
            info,
        };

        let mut tokens = TokenStream::new();
        scope_type.to_tokens(&mut tokens);
        let token_str = tokens.to_string();

        assert!(token_str.contains("pub enum EntityScope"));
        assert!(token_str.contains("PartnerId (PartnerId)"));
        assert!(token_str.contains("CustomerId (CustomerId)"));
        assert!(token_str.contains("impl From < PartnerId > for EntityScope"));
        assert!(token_str.contains("impl From < CustomerId > for EntityScope"));
        assert!(token_str.contains("impl From < & CustomerId > for EntityScope"));
    }

    #[test]
    fn scope_col_predicate() {
        let column_name = syn::Ident::new("partner_id", Span::call_site());
        let column_ty: syn::Type = syn::parse_str("PartnerId").unwrap();
        let (info, _entity) = test_info(&[(&column_name, &column_ty)]);

        assert_eq!(info.cols[0].predicate(1), "partner_id = $1");
        assert_eq!(info.cols[0].predicate(3), "partner_id = $3");
    }

    #[test]
    fn dispatch_emits_one_arm_per_column() {
        let partner = syn::Ident::new("partner_id", Span::call_site());
        let partner_ty: syn::Type = syn::parse_str("PartnerId").unwrap();
        let customer = syn::Ident::new("customer_id", Span::call_site());
        let customer_ty: syn::Type = syn::parse_str("CustomerId").unwrap();
        let (info, _entity) = test_info(&[(&partner, &partner_ty), (&customer, &customer_ty)]);

        let tokens = info
            .dispatch(quote! { all() }, |col| {
                let p = col.predicate(2);
                quote! { scoped(#p) }
            })
            .to_string();
        assert!(tokens.contains("EntityScope :: All => all ()"));
        assert!(
            tokens
                .contains("EntityScope :: PartnerId (__scope_val) => scoped (\"partner_id = $2\")")
        );
        assert!(
            tokens.contains(
                "EntityScope :: CustomerId (__scope_val) => scoped (\"customer_id = $2\")"
            )
        );
    }
}
