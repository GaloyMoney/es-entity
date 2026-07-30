use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::*;

/// Info about a repo's scope column, shared by the read-fn emitters.
///
/// A repo with a column marked `scope` generates every read fn
/// (`find_by_*`, `find_all`, `list_by_*`, `list_for_*`, `list_for_filters*`)
/// with a leading `scope: impl Into<{Entity}Scope>` argument. At runtime the
/// fn dispatches on the scope: `All` executes the exact same SQL as an
/// unscoped repo, `Only(value)` executes a variant with an additional
/// `scope_column = $n` conjunct — both static, sargable `es_query!` literals.
#[derive(Clone)]
pub struct ScopeInfo<'a> {
    /// The generated scope enum ident: `{Entity}Scope`.
    pub scope_ty: syn::Ident,
    pub column_name: &'a syn::Ident,
    pub column_ty: &'a syn::Type,
}

impl<'a> ScopeInfo<'a> {
    pub fn from_opts(opts: &'a RepositoryOptions) -> Option<Self> {
        opts.columns.scope_column().map(|col| ScopeInfo {
            scope_ty: opts.scope_type_ident(),
            column_name: col.name(),
            column_ty: col.ty(),
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

    /// The SQL conjunct for the `Only` arm at the given parameter index.
    pub fn predicate(&self, param_idx: u32) -> String {
        format!("{} = ${}", self.column_name, param_idx)
    }

    /// The query binding for the `Only` arm (pairs with [`Self::dispatch`]'s
    /// `__scope_val` pattern binding).
    pub fn arg_tokens(&self) -> TokenStream {
        let column_ty = self.column_ty;
        quote! { __scope_val as &#column_ty, }
    }

    /// Runtime dispatch between the unscoped (`All`) and scoped (`Only`)
    /// query variants. The `Only` arm binds `__scope_val: &T`.
    pub fn dispatch(&self, all_arm: TokenStream, only_arm: TokenStream) -> TokenStream {
        let scope_ty = &self.scope_ty;
        quote! {
            match &__scope {
                #scope_ty::All => #all_arm,
                #scope_ty::Only(__scope_val) => #only_arm,
            }
        }
    }
}

/// Generates the per-repo scope enum:
///
/// ```ignore
/// pub enum UserScope {
///     All,
///     Only(PartnerId),
/// }
/// ```
///
/// plus `From<T>` / `From<&T>` conversions into `Only` so call sites can pass
/// a scope value directly. Deliberately **no** `From<Option<T>>`: mapping
/// `None` to `All` would turn a stray `None` into silent all-scope access.
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
        let column_ty = self.info.column_ty;
        let doc = format!(
            "Scope argument for [`{}`] repository reads: `Only(value)` filters every query by \
             the scope column, `All` reads across all scopes (audited escape hatch).",
            self.entity,
        );

        tokens.append_all(quote! {
            #[doc = #doc]
            #[derive(Debug, Clone, Copy)]
            pub enum #scope_ty {
                /// No scope filter — reads across all scopes.
                All,
                /// Restricts every read to rows whose scope column equals the value.
                Only(#column_ty),
            }

            impl From<#column_ty> for #scope_ty {
                fn from(value: #column_ty) -> Self {
                    #scope_ty::Only(value)
                }
            }

            impl From<&#column_ty> for #scope_ty {
                fn from(value: &#column_ty) -> Self {
                    #scope_ty::Only(*value)
                }
            }

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
    use proc_macro2::Span;

    fn test_info<'a>(
        column_name: &'a syn::Ident,
        column_ty: &'a syn::Type,
    ) -> (ScopeInfo<'a>, syn::Ident) {
        let entity = syn::Ident::new("Entity", Span::call_site());
        (
            ScopeInfo {
                scope_ty: syn::Ident::new("EntityScope", Span::call_site()),
                column_name,
                column_ty,
            },
            entity,
        )
    }

    #[test]
    fn scope_type_tokens() {
        let column_name = syn::Ident::new("partner_id", Span::call_site());
        let column_ty: syn::Type = syn::parse_str("PartnerId").unwrap();
        let (info, entity) = test_info(&column_name, &column_ty);
        let scope_type = ScopeType {
            entity: &entity,
            info,
        };

        let mut tokens = TokenStream::new();
        scope_type.to_tokens(&mut tokens);
        let token_str = tokens.to_string();

        assert!(token_str.contains("pub enum EntityScope"));
        assert!(token_str.contains("Only (PartnerId)"));
        assert!(token_str.contains("impl From < PartnerId > for EntityScope"));
        assert!(token_str.contains("impl From < & PartnerId > for EntityScope"));
        assert!(!token_str.contains("Option"));
    }

    #[test]
    fn scope_info_predicate() {
        let column_name = syn::Ident::new("partner_id", Span::call_site());
        let column_ty: syn::Type = syn::parse_str("PartnerId").unwrap();
        let (info, _entity) = test_info(&column_name, &column_ty);

        assert_eq!(info.predicate(1), "partner_id = $1");
        assert_eq!(info.predicate(3), "partner_id = $3");
    }
}
