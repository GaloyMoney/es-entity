use darling::{FromMeta, util::Flag};

use super::ScopeOpts;

/// How a repo resolves the id of an entity it creates.
#[derive(Debug, Clone)]
pub enum IdPolicyOpt {
    /// The repo mints the id (`<Id>::new()`, or the `by` generator). The
    /// default.
    Generated { by: Option<syn::Path> },
    /// The `New*` type brings its own id.
    Supplied,
}

impl Default for IdPolicyOpt {
    fn default() -> Self {
        Self::Generated { by: None }
    }
}

impl IdPolicyOpt {
    pub fn is_generated(&self) -> bool {
        matches!(self, Self::Generated { .. })
    }

    /// The `es_entity` policy marker the `New*` type must name as its
    /// `IntoEvents::Id`.
    pub fn marker(&self) -> proc_macro2::TokenStream {
        match self {
            Self::Generated { .. } => quote::quote!(es_entity::Generated),
            Self::Supplied => quote::quote!(es_entity::Supplied),
        }
    }

    /// The builder handed to `into_events`, given an expression for the
    /// resolved id (ignored by a supplied policy).
    pub fn builder(&self, id: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
        match self {
            Self::Generated { .. } => {
                quote::quote!(es_entity::EntityEventsBuilder::__generated(#id))
            }
            Self::Supplied => quote::quote!(es_entity::EntityEventsBuilder::__supplied()),
        }
    }

    /// An expression resolving a fresh id inside an async fn that holds
    /// `self` and `op`: the id type's `new()`, or the repo's `by` generator,
    /// an `async fn(&Repo, &mut impl AtomicOperation) -> Result<Id, RepoFault>`.
    pub fn resolve(&self, id_ty: &syn::Ident) -> proc_macro2::TokenStream {
        match self {
            Self::Generated { by: Some(path) } => {
                quote::quote!(#path(self, &mut *op).await?)
            }
            _ => quote::quote!(#id_ty::new()),
        }
    }
}

/// The repo-level `id` attribute, in the same two forms as a column:
/// `id = "CustomId"` names the type and keeps every default, and
/// `id(ty = "CustomId", supplied, scope)` spells the options out. Unlike a
/// column, `ty` is optional (it defaults to `{Entity}Id`), `find_by` and
/// `list_by` are always on, and the id alone takes a policy: `generated` (the
/// default), `generated(by = "path::to::fn")` or `supplied`.
#[derive(Debug, Default)]
pub struct IdOpts {
    pub ty: Option<syn::Ident>,
    pub policy: IdPolicyOpt,
    pub scope: Option<ScopeOpts>,
}

impl FromMeta for IdOpts {
    fn from_string(value: &str) -> darling::Result<Self> {
        Ok(Self {
            ty: Some(syn::Ident::from_string(value)?),
            ..Self::default()
        })
    }

    fn from_list(items: &[darling::ast::NestedMeta]) -> darling::Result<Self> {
        #[derive(FromMeta)]
        struct Inner {
            #[darling(default)]
            ty: Option<syn::Ident>,
            #[darling(default)]
            supplied: Flag,
            #[darling(default)]
            generated: Option<GeneratedOpts>,
            #[darling(default)]
            scope: Option<ScopeOpts>,
        }

        let inner = Inner::from_list(items)?;
        let policy = match (inner.supplied.is_present(), inner.generated) {
            (true, Some(_)) => {
                return Err(darling::Error::custom(
                    "`supplied` and `generated` are mutually exclusive",
                )
                .with_span(&inner.supplied.span()));
            }
            (true, None) => IdPolicyOpt::Supplied,
            (false, generated) => IdPolicyOpt::Generated {
                by: generated.and_then(|g| g.by),
            },
        };
        Ok(Self {
            ty: inner.ty,
            policy,
            scope: inner.scope,
        })
    }
}

#[derive(Debug, Default)]
struct GeneratedOpts {
    by: Option<syn::Path>,
}

impl FromMeta for GeneratedOpts {
    fn from_word() -> darling::Result<Self> {
        Ok(Self::default())
    }

    fn from_list(items: &[darling::ast::NestedMeta]) -> darling::Result<Self> {
        #[derive(FromMeta)]
        struct Inner {
            by: syn::Path,
        }

        let inner = Inner::from_list(items)?;
        Ok(Self { by: Some(inner.by) })
    }
}

#[cfg(test)]
mod tests {
    use darling::FromMeta;
    use syn::parse_quote;

    use super::*;

    fn parse(meta: syn::Meta) -> darling::Result<IdOpts> {
        IdOpts::from_meta(&meta)
    }

    fn err(meta: syn::Meta) -> String {
        match parse(meta) {
            Err(err) => err.to_string(),
            Ok(opts) => panic!("expected an error, got {opts:?}"),
        }
    }

    #[test]
    fn short_form_names_the_type_and_keeps_the_defaults() {
        let opts = parse(parse_quote!(id = "CustomId")).unwrap();
        assert_eq!(opts.ty.unwrap().to_string(), "CustomId");
        assert!(matches!(opts.policy, IdPolicyOpt::Generated { by: None }));
        assert!(opts.scope.is_none());
    }

    #[test]
    fn empty_long_form_is_the_default() {
        let opts = parse(parse_quote!(id())).unwrap();
        assert!(opts.ty.is_none());
        assert!(matches!(opts.policy, IdPolicyOpt::Generated { by: None }));
        assert!(opts.scope.is_none());
    }

    #[test]
    fn long_form_takes_ty_policy_and_scope() {
        let opts = parse(parse_quote!(id(ty = "CustomId", supplied, scope))).unwrap();
        assert_eq!(opts.ty.unwrap().to_string(), "CustomId");
        assert!(matches!(opts.policy, IdPolicyOpt::Supplied));
        assert!(opts.scope.is_some());
    }

    #[test]
    fn generated_is_explicit_or_takes_a_generator() {
        let opts = parse(parse_quote!(id(generated))).unwrap();
        assert!(matches!(opts.policy, IdPolicyOpt::Generated { by: None }));

        let opts = parse(parse_quote!(id(generated(by = "Self::next_id")))).unwrap();
        let IdPolicyOpt::Generated { by: Some(by) } = opts.policy else {
            panic!("expected a generator");
        };
        assert_eq!(quote::quote!(#by).to_string(), "Self :: next_id");
    }

    #[test]
    fn supplied_and_generated_are_exclusive() {
        let err = err(parse_quote!(id(supplied, generated)));
        assert!(
            err.contains("mutually exclusive"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn generated_list_requires_by() {
        let err = err(parse_quote!(id(generated(other = "x"))));
        assert!(
            err.contains("other") || err.contains("by"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn scope_variant_override_is_parsed() {
        let opts = parse(parse_quote!(id(scope(variant = "Tenant")))).unwrap();
        assert!(opts.scope.is_some());
    }

    #[test]
    fn scope_variant_override_must_be_valid_identifier() {
        let err = err(parse_quote!(id(scope(variant = "not an ident"))));
        assert!(
            err.contains("not a valid Rust identifier"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn scope_rejects_unknown_scope_option() {
        let err = err(parse_quote!(id(scope(unknown = "x"))));
        assert!(
            err.contains("unknown") || err.contains("Unknown"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn scope_rejects_name_value_form() {
        assert!(parse(parse_quote!(id(scope = true))).is_err());
    }

    #[test]
    fn rejects_column_options_the_id_does_not_take() {
        for meta in [
            parse_quote!(id(find_by = false)),
            parse_quote!(id(list_by = false)),
            parse_quote!(id(nullable)),
            parse_quote!(id(list_for)),
            parse_quote!(id(create(persist = false))),
            parse_quote!(id(update(persist = true))),
            parse_quote!(id(parent)),
        ] {
            let err = err(meta);
            assert!(err.contains("Unknown field"), "unexpected error: {err}");
        }
    }
}
