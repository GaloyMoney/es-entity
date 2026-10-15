use darling::FromMeta;

use super::ScopeOpts;

/// Where a repo gets the id of an entity it creates.
#[derive(Debug, Clone)]
pub enum IdSourceOpt {
    /// The repo generates the id (`<Id>::new()`, or the `generator`). The
    /// default.
    Repo { generator: Option<syn::Path> },
    /// The caller puts the id in the `New*` value.
    Caller,
}

impl Default for IdSourceOpt {
    fn default() -> Self {
        Self::Repo { generator: None }
    }
}

impl IdSourceOpt {
    pub fn is_repo(&self) -> bool {
        matches!(self, Self::Repo { .. })
    }

    /// The `es_entity` id source marker the `New*` type must name as its
    /// `IntoEvents::IdSource`.
    pub fn marker(&self) -> proc_macro2::TokenStream {
        match self {
            Self::Repo { .. } => quote::quote!(es_entity::FromRepo),
            Self::Caller => quote::quote!(es_entity::FromCaller),
        }
    }

    /// The builder handed to `into_events`, given an expression for the
    /// resolved id (ignored when the caller is the source).
    pub fn builder(&self, id: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
        match self {
            Self::Repo { .. } => {
                quote::quote!(es_entity::EntityEventsBuilder::__from_repo(#id))
            }
            Self::Caller => quote::quote!(es_entity::EntityEventsBuilder::__from_caller()),
        }
    }

    /// An expression resolving a fresh id inside an async fn that holds
    /// `self` and `op`: the id type's `new()`, or the repo's `generator`, an
    /// `async fn(&Repo, &mut impl AtomicOperation) -> Result<Id, RepoFault>`.
    pub fn resolve(&self, id_ty: &syn::Ident) -> proc_macro2::TokenStream {
        match self {
            Self::Repo {
                generator: Some(path),
            } => {
                quote::quote!(#path(self, &mut *op).await?)
            }
            _ => quote::quote!(#id_ty::new()),
        }
    }
}

/// The `source = repo | caller` value, written as a bare word or a string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    Repo,
    Caller,
}

impl FromMeta for SourceKind {
    fn from_string(value: &str) -> darling::Result<Self> {
        match value {
            "repo" => Ok(Self::Repo),
            "caller" => Ok(Self::Caller),
            other => Err(darling::Error::unknown_value(other)),
        }
    }

    fn from_expr(expr: &syn::Expr) -> darling::Result<Self> {
        match expr {
            syn::Expr::Path(path) => match path.path.get_ident() {
                Some(ident) => {
                    Self::from_string(&ident.to_string()).map_err(|e| e.with_span(ident))
                }
                None => Err(darling::Error::unexpected_expr_type(expr)),
            },
            syn::Expr::Lit(lit) => Self::from_value(&lit.lit),
            syn::Expr::Group(group) => Self::from_expr(&group.expr),
            _ => Err(darling::Error::unexpected_expr_type(expr)),
        }
    }
}

/// The repo-level `id` attribute, in the same two forms as a column:
/// `id = "CustomId"` names the type and keeps every default, and
/// `id(ty = "CustomId", source = caller, scope)` spells the options out.
/// Unlike a column, `ty` is optional (it defaults to `{Entity}Id`), `find_by`
/// and `list_by` are always on, and the id alone takes a source: `repo` (the
/// default, optionally with `generator = "path::to::fn"`) or `caller`.
#[derive(Debug, Default)]
pub struct IdOpts {
    pub ty: Option<syn::Ident>,
    pub source: IdSourceOpt,
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
            source: Option<darling::util::SpannedValue<SourceKind>>,
            #[darling(default)]
            generator: Option<darling::util::SpannedValue<syn::Path>>,
            #[darling(default)]
            scope: Option<ScopeOpts>,
        }

        let inner = Inner::from_list(items)?;
        let source = match (inner.source.as_deref(), inner.generator) {
            (Some(SourceKind::Caller), Some(generator)) => {
                return Err(darling::Error::custom(
                    "`generator` needs `source = repo`: a caller-sourced id is not generated",
                )
                .with_span(&generator.span()));
            }
            (Some(SourceKind::Caller), None) => IdSourceOpt::Caller,
            (_, generator) => IdSourceOpt::Repo {
                generator: generator.map(|g| g.into_inner()),
            },
        };
        Ok(Self {
            ty: inner.ty,
            source,
            scope: inner.scope,
        })
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
        assert!(matches!(opts.source, IdSourceOpt::Repo { generator: None }));
        assert!(opts.scope.is_none());
    }

    #[test]
    fn empty_long_form_is_the_default() {
        let opts = parse(parse_quote!(id())).unwrap();
        assert!(opts.ty.is_none());
        assert!(matches!(opts.source, IdSourceOpt::Repo { generator: None }));
        assert!(opts.scope.is_none());
    }

    #[test]
    fn long_form_takes_ty_source_and_scope() {
        let opts = parse(parse_quote!(id(ty = "CustomId", source = caller, scope))).unwrap();
        assert_eq!(opts.ty.unwrap().to_string(), "CustomId");
        assert!(matches!(opts.source, IdSourceOpt::Caller));
        assert!(opts.scope.is_some());
    }

    #[test]
    fn source_takes_a_bare_word_or_a_string() {
        let opts = parse(parse_quote!(id(source = "caller"))).unwrap();
        assert!(matches!(opts.source, IdSourceOpt::Caller));

        let opts = parse(parse_quote!(id(source = "repo"))).unwrap();
        assert!(matches!(opts.source, IdSourceOpt::Repo { generator: None }));
    }

    #[test]
    fn repo_source_is_explicit_or_takes_a_generator() {
        let opts = parse(parse_quote!(id(source = repo))).unwrap();
        assert!(matches!(opts.source, IdSourceOpt::Repo { generator: None }));

        for meta in [
            parse_quote!(id(source = repo, generator = "Self::next_id")),
            parse_quote!(id(generator = "Self::next_id")),
        ] {
            let opts = parse(meta).unwrap();
            let IdSourceOpt::Repo {
                generator: Some(generator),
            } = opts.source
            else {
                panic!("expected a generator");
            };
            assert_eq!(quote::quote!(#generator).to_string(), "Self :: next_id");
        }
    }

    #[test]
    fn generator_needs_the_repo_source() {
        let err = err(parse_quote!(id(
            source = caller,
            generator = "Self::next_id"
        )));
        assert!(
            err.contains("`generator` needs `source = repo`"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn source_rejects_an_unknown_value() {
        let err = err(parse_quote!(id(source = database)));
        assert!(err.contains("database"), "unexpected error: {err}");
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
            parse_quote!(id(supplied)),
            parse_quote!(id(generated)),
        ] {
            let err = err(meta);
            assert!(err.contains("Unknown field"), "unexpected error: {err}");
        }
    }
}
