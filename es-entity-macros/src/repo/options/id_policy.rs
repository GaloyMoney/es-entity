use darling::FromMeta;

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
}

/// The `id` attribute: a string names the id type (`id = "CustomId"`); a bare
/// `supplied`, `generated` or `generated(by = "path::to::fn")` selects the
/// policy.
#[derive(Debug, Clone)]
pub enum IdAttr {
    Type(syn::Ident),
    Policy(IdPolicyOpt),
}

impl FromMeta for IdAttr {
    fn from_string(value: &str) -> darling::Result<Self> {
        syn::parse_str::<syn::Ident>(value)
            .map(Self::Type)
            .map_err(|e| darling::Error::custom(e.to_string()))
    }

    fn from_expr(expr: &syn::Expr) -> darling::Result<Self> {
        match expr {
            syn::Expr::Lit(lit) => Self::from_value(&lit.lit),
            syn::Expr::Group(group) => Self::from_expr(&group.expr),
            syn::Expr::Path(path) if path.path.is_ident("supplied") => {
                Ok(Self::Policy(IdPolicyOpt::Supplied))
            }
            syn::Expr::Path(path) if path.path.is_ident("generated") => {
                Ok(Self::Policy(IdPolicyOpt::Generated { by: None }))
            }
            syn::Expr::Call(call) => generated_by(call).map(Self::Policy),
            other => Err(darling::Error::custom(
                "expected an id type name, `supplied`, `generated` or `generated(by = \"path\")`",
            )
            .with_span(other)),
        }
    }
}

fn generated_by(call: &syn::ExprCall) -> darling::Result<IdPolicyOpt> {
    let is_generated =
        matches!(&*call.func, syn::Expr::Path(path) if path.path.is_ident("generated"));
    let mut args = call.args.iter();
    if let (true, Some(syn::Expr::Assign(assign)), None) = (is_generated, args.next(), args.next())
        && matches!(&*assign.left, syn::Expr::Path(path) if path.path.is_ident("by"))
        && let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(path),
            ..
        }) = &*assign.right
    {
        return Ok(IdPolicyOpt::Generated {
            by: Some(path.parse().map_err(darling::Error::from)?),
        });
    }
    Err(darling::Error::custom("expected `generated(by = \"path::to::fn\")`").with_span(call))
}

impl IdPolicyOpt {
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
