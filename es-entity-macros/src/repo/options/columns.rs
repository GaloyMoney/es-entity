use darling::FromMeta;
use quote::quote;

#[derive(Default)]
pub struct Columns {
    all: Vec<Column>,
}

impl Columns {
    #[cfg(test)]
    pub fn new(id: &syn::Ident, columns: impl IntoIterator<Item = Column>) -> Self {
        let all = columns.into_iter().collect();
        let mut res = Columns { all };
        res.set_id_column(id);
        res
    }

    pub fn set_id_column(&mut self, ty: &syn::Ident) {
        let mut all = vec![
            Column::for_created_at(),
            Column::for_id(syn::parse_str(&ty.to_string()).unwrap()),
        ];
        all.append(&mut self.all);
        self.all = all;
    }

    pub fn all_find_by(&self) -> impl Iterator<Item = &Column> {
        self.all.iter().filter(|c| c.opts.find_by())
    }

    pub fn all_list_by(&self) -> impl Iterator<Item = &Column> {
        self.all.iter().filter(|c| c.opts.list_by())
    }

    pub fn all_list_for(&self) -> impl Iterator<Item = &Column> {
        self.all.iter().filter(|c| c.opts.list_for())
    }

    pub fn find_list_by(&self, name: &syn::Ident) -> Option<&Column> {
        self.all
            .iter()
            .find(|c| c.name() == name && c.opts.list_by())
    }

    pub fn validate_list_for_by_columns(&self) -> darling::Result<()> {
        let mut errors = darling::Error::accumulator();
        for col in self.all.iter().filter(|c| c.opts.list_for()) {
            for by_name in col.list_for_by_columns() {
                if self.find_list_by(by_name).is_none() {
                    let available: Vec<_> =
                        self.all_list_by().map(|c| c.name().to_string()).collect();
                    errors.push(darling::Error::custom(format!(
                        "column '{}' in list_for(by(...)) on '{}' is not a list_by column. Available list_by columns: {}",
                        by_name,
                        col.name(),
                        available.join(", "),
                    )));
                }
            }
        }
        errors.finish()
    }

    pub fn parent(&self) -> Option<&Column> {
        self.all.iter().find(|c| c.opts.parent_opts.is_some())
    }

    pub fn updates_needed(&self) -> bool {
        self.all.iter().any(|c| c.opts.persist_on_update())
    }

    pub fn variable_assignments_for_update(&self, ident: syn::Ident) -> proc_macro2::TokenStream {
        let assignments = self.all.iter().filter_map(|c| {
            if c.opts.persist_on_update() || c.opts.is_id {
                Some(c.variable_assignment_for_update(&ident))
            } else {
                None
            }
        });
        quote! {
            #(#assignments)*
        }
    }

    pub fn variable_assignments_for_create(&self, ident: syn::Ident) -> proc_macro2::TokenStream {
        let assignments = self.all.iter().filter_map(|c| {
            if c.opts.persist_on_create() {
                Some(c.variable_assignment_for_create(&ident))
            } else {
                None
            }
        });
        quote! {
            #(#assignments)*
        }
    }

    pub fn variable_assignments_for_create_all(
        &self,
        ident: syn::Ident,
    ) -> proc_macro2::TokenStream {
        let assignments = self.all.iter().filter_map(|c| {
            if c.opts.persist_on_create() {
                Some(c.variable_assignment_for_create_all(&ident))
            } else {
                None
            }
        });
        quote! {
            #(#assignments)*
        }
    }

    pub fn create_query_args(&self) -> Vec<proc_macro2::TokenStream> {
        self.all
            .iter()
            .filter(|c| c.opts.persist_on_create())
            .map(|column| {
                let ident = &column.name;
                let ty = &column.opts.ty;
                quote! {
                    #ident as &#ty,
                }
            })
            .collect()
    }

    pub fn create_all_arg_collection(
        &self,
        ident: syn::Ident,
    ) -> (proc_macro2::TokenStream, Vec<proc_macro2::TokenStream>) {
        let assignments = self.variable_assignments_for_create_all(ident.clone());
        let (vecs, pushes, bindings) = self
            .all
            .iter()
            .filter(|c| c.opts.persist_on_create())
            .map(|column| {
                let vec_ident = syn::Ident::new(
                    &format!("{}_collection", column.name),
                    proc_macro2::Span::call_site(),
                );
                let ident = &column.name;
                (
                    quote! {
                        let mut #vec_ident = Vec::new();
                    },
                    quote! {
                        #vec_ident.push(#ident);
                    },
                    quote! {
                        .bind(#vec_ident)
                    },
                )
            })
            .fold(
                (Vec::new(), Vec::new(), Vec::new()),
                |(mut v1, mut v2, mut v3): (Vec<_>, Vec<_>, Vec<_>), (a, b, c)| {
                    v1.push(a);
                    v2.push(b);
                    v3.push(c);
                    (v1, v2, v3)
                },
            );
        (
            quote! {
                #(#vecs)*
                for #ident in new_entities.iter() {
                    #assignments

                    #(#pushes)*
                }
            },
            bindings,
        )
    }

    pub fn insert_column_names(&self) -> Vec<String> {
        self.all
            .iter()
            .filter_map(|c| {
                if c.opts.persist_on_create() {
                    Some(c.name.to_string())
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn insert_placeholders(&self, offset: usize) -> String {
        let count = self
            .all
            .iter()
            .filter(|c| c.opts.persist_on_create())
            .count();
        ((1 + offset)..=(count + offset))
            .map(|i| format!("${i}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn sql_updates(&self) -> String {
        self.all
            .iter()
            .skip(1)
            .filter(|c| c.opts.persist_on_update())
            .enumerate()
            .map(|(idx, column)| format!("{} = ${}", column.name, idx + 2))
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn update_query_args(&self) -> Vec<proc_macro2::TokenStream> {
        self.all
            .iter()
            .filter(|c| c.opts.persist_on_update() || c.opts.is_id)
            .map(|column| {
                let ident = &column.name;
                let ty = &column.opts.ty;
                quote! {
                    #ident as &#ty
                }
            })
            .collect()
    }

    pub fn update_all_arg_parts(
        &self,
        ident: syn::Ident,
    ) -> (
        proc_macro2::TokenStream,
        proc_macro2::TokenStream,
        Vec<proc_macro2::TokenStream>,
    ) {
        let assignments = {
            let assignments = self.all.iter().filter_map(|c| {
                if c.opts.persist_on_update() || c.opts.is_id {
                    Some(c.variable_assignment_for_update_all(&ident))
                } else {
                    None
                }
            });
            quote! {
                #(#assignments)*
            }
        };
        let (vecs, pushes, bindings) = self
            .all
            .iter()
            .filter(|c| c.opts.persist_on_update() || c.opts.is_id)
            .map(|column| {
                let vec_ident = syn::Ident::new(
                    &format!("{}_collection", column.name),
                    proc_macro2::Span::call_site(),
                );
                let ident = &column.name;
                (
                    quote! {
                        let mut #vec_ident = Vec::new();
                    },
                    quote! {
                        #vec_ident.push(#ident);
                    },
                    quote! {
                        .bind(#vec_ident)
                    },
                )
            })
            .fold(
                (Vec::new(), Vec::new(), Vec::new()),
                |(mut v1, mut v2, mut v3): (Vec<_>, Vec<_>, Vec<_>), (a, b, c)| {
                    v1.push(a);
                    v2.push(b);
                    v3.push(c);
                    (v1, v2, v3)
                },
            );
        (
            quote! { #(#vecs)* },
            quote! {
                #assignments
                #(#pushes)*
            },
            bindings,
        )
    }

    pub fn sql_bulk_update_set(&self) -> String {
        self.all
            .iter()
            .filter(|c| c.opts.persist_on_update() && !c.opts.is_id)
            .map(|column| format!("{name} = unnested.{name}", name = column.name))
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn update_all_column_names(&self) -> Vec<String> {
        self.all
            .iter()
            .filter(|c| c.opts.persist_on_update() || c.opts.is_id)
            .map(|c| c.name.to_string())
            .collect()
    }
}

impl FromMeta for Columns {
    fn from_list(items: &[darling::ast::NestedMeta]) -> darling::Result<Self> {
        let all = items
            .iter()
            .map(Column::from_nested_meta)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Columns { all })
    }
}

#[derive(PartialEq)]
pub struct Column {
    name: syn::Ident,
    opts: ColumnOpts,
}

impl FromMeta for Column {
    fn from_nested_meta(item: &darling::ast::NestedMeta) -> darling::Result<Self> {
        match item {
            darling::ast::NestedMeta::Meta(
                meta @ syn::Meta::NameValue(syn::MetaNameValue {
                    value:
                        syn::Expr::Lit(syn::ExprLit {
                            lit: syn::Lit::Str(lit_str),
                            ..
                        }),
                    ..
                }),
            ) => {
                let name = meta.path().get_ident().cloned().ok_or_else(|| {
                    darling::Error::custom("Expected identifier").with_span(meta.path())
                })?;
                Ok(Column::new(name, syn::parse_str(&lit_str.value())?))
            }
            darling::ast::NestedMeta::Meta(meta @ syn::Meta::List(_)) => {
                let name = meta.path().get_ident().cloned().ok_or_else(|| {
                    darling::Error::custom("Expected identifier").with_span(meta.path())
                })?;
                let column = Column {
                    name,
                    opts: ColumnOpts::from_meta(meta)?,
                };
                Ok(column)
            }
            _ => Err(
                darling::Error::custom("Expected name-value pair or attribute list")
                    .with_span(item),
            ),
        }
    }
}

impl Column {
    pub fn new(name: syn::Ident, ty: syn::Type) -> Self {
        Column {
            name,
            opts: ColumnOpts::new(ty),
        }
    }

    #[cfg(test)]
    pub fn new_list_for(name: syn::Ident, ty: syn::Type, by_columns: Vec<syn::Ident>) -> Self {
        Column {
            name,
            opts: ColumnOpts {
                list_for_opts: Some(ListForOpts { by_columns }),
                ..ColumnOpts::new(ty)
            },
        }
    }

    pub fn for_id(ty: syn::Type) -> Self {
        Column {
            name: syn::Ident::new("id", proc_macro2::Span::call_site()),
            opts: ColumnOpts {
                ty,
                is_id: true,
                list_by: Some(true),
                find_by: Some(true),
                list_for_opts: None,
                parent_opts: None,
                create_opts: Some(CreateOpts {
                    persist: Some(true),
                    accessor: None,
                }),
                update_opts: Some(UpdateOpts {
                    persist: Some(false),
                    accessor: None,
                }),
            },
        }
    }

    pub fn for_created_at() -> Self {
        Column {
            name: syn::Ident::new("created_at", proc_macro2::Span::call_site()),
            opts: ColumnOpts {
                ty: syn::parse_quote!(
                    es_entity::prelude::chrono::DateTime<es_entity::prelude::chrono::Utc>
                ),
                is_id: false,
                list_by: Some(true),
                find_by: Some(false),
                list_for_opts: None,
                parent_opts: None,
                create_opts: Some(CreateOpts {
                    persist: Some(false),
                    accessor: None,
                }),
                update_opts: Some(UpdateOpts {
                    persist: Some(false),
                    accessor: Some(syn::parse_quote!(
                        events()
                            .entity_first_persisted_at()
                            .expect("entity not persisted")
                    )),
                }),
            },
        }
    }

    pub fn list_for_by_columns(&self) -> &[syn::Ident] {
        self.opts.list_for_by_columns()
    }

    pub fn is_id(&self) -> bool {
        self.opts.is_id
    }

    pub fn is_optional(&self) -> bool {
        if let syn::Type::Path(type_path) = self.ty()
            && type_path.path.segments.len() == 1
        {
            let segment = &type_path.path.segments[0];
            if segment.ident == "Option" {
                return true;
            }
        }
        false
    }

    pub fn name(&self) -> &syn::Ident {
        &self.name
    }

    pub fn ty(&self) -> &syn::Type {
        &self.opts.ty
    }

    pub fn ty_for_find_by(
        &self,
    ) -> (
        syn::Type,
        proc_macro2::TokenStream,
        proc_macro2::TokenStream,
    ) {
        if let syn::Type::Path(type_path) = self.ty()
            && type_path.path.is_ident("String")
        {
            (
                syn::parse_quote! { str },
                quote! { impl std::convert::AsRef<str> },
                quote! { as_ref() },
            )
        } else {
            let ty = &self.ty();
            (
                self.ty().clone(),
                quote! { impl std::borrow::Borrow<#ty> },
                quote! { borrow() },
            )
        }
    }

    pub fn accessor(&self) -> proc_macro2::TokenStream {
        self.opts.update_accessor(&self.name)
    }

    pub fn parent_accessor(&self) -> proc_macro2::TokenStream {
        self.opts.parent_accessor(&self.name)
    }

    fn variable_assignment_for_create(&self, ident: &syn::Ident) -> proc_macro2::TokenStream {
        let name = &self.name;
        let accessor = self.opts.create_accessor(name);
        quote! {
            let #name = &#ident.#accessor;
        }
    }

    fn variable_assignment_for_create_all(&self, ident: &syn::Ident) -> proc_macro2::TokenStream {
        let name = &self.name;
        let accessor = self.opts.create_accessor(name);
        let ty = &self.opts.ty;
        if self.opts.create_accessor_returns_owned() {
            quote! {
                let #name: #ty = #ident.#accessor;
            }
        } else {
            quote! {
                let #name: &#ty = &#ident.#accessor;
            }
        }
    }

    fn variable_assignment_for_update(&self, ident: &syn::Ident) -> proc_macro2::TokenStream {
        let name = &self.name;
        let accessor = self.opts.update_accessor(name);
        quote! {
            let #name = &#ident.#accessor;
        }
    }

    fn variable_assignment_for_update_all(&self, ident: &syn::Ident) -> proc_macro2::TokenStream {
        let name = &self.name;
        let accessor = self.opts.update_accessor(name);
        if self.opts.update_accessor_returns_owned() {
            quote! {
                let #name = #ident.#accessor;
            }
        } else {
            quote! {
                let #name = &#ident.#accessor;
            }
        }
    }
}

#[derive(PartialEq, FromMeta)]
struct ColumnOpts {
    ty: syn::Type,
    #[darling(default, skip)]
    is_id: bool,
    #[darling(default)]
    find_by: Option<bool>,
    #[darling(default)]
    list_by: Option<bool>,
    #[darling(default, rename = "list_for")]
    list_for_opts: Option<ListForOpts>,
    #[darling(default, rename = "parent")]
    parent_opts: Option<ParentOpts>,
    #[darling(default, rename = "create")]
    create_opts: Option<CreateOpts>,
    #[darling(default, rename = "update")]
    update_opts: Option<UpdateOpts>,
}

impl ColumnOpts {
    fn new(ty: syn::Type) -> Self {
        ColumnOpts {
            ty,
            is_id: false,
            find_by: None,
            list_by: None,
            list_for_opts: None,
            parent_opts: None,
            create_opts: None,
            update_opts: None,
        }
    }

    fn find_by(&self) -> bool {
        self.find_by.unwrap_or(true)
    }

    fn list_by(&self) -> bool {
        self.list_by.unwrap_or(false)
    }

    fn list_for(&self) -> bool {
        self.list_for_opts.is_some()
    }

    fn list_for_by_columns(&self) -> &[syn::Ident] {
        self.list_for_opts
            .as_ref()
            .map(|o| o.by_columns.as_slice())
            .unwrap_or(&[])
    }

    fn persist_on_create(&self) -> bool {
        self.create_opts
            .as_ref()
            .is_none_or(|o| o.persist.unwrap_or(true))
    }

    fn create_accessor(&self, name: &syn::Ident) -> proc_macro2::TokenStream {
        if let Some(accessor) = &self.create_opts.as_ref().and_then(|o| o.accessor.as_ref()) {
            quote! {
                #accessor
            }
        } else {
            quote! {
                #name
            }
        }
    }

    fn persist_on_update(&self) -> bool {
        self.update_opts
            .as_ref()
            .is_none_or(|o| o.persist.unwrap_or(true))
    }

    fn update_accessor(&self, name: &syn::Ident) -> proc_macro2::TokenStream {
        if let Some(accessor) = &self.update_opts.as_ref().and_then(|o| o.accessor.as_ref()) {
            quote! {
                #accessor
            }
        } else {
            quote! {
                #name
            }
        }
    }

    fn create_accessor_returns_owned(&self) -> bool {
        self.create_opts
            .as_ref()
            .and_then(|o| o.accessor.as_ref())
            .is_some_and(|expr| matches!(expr, syn::Expr::Call(_) | syn::Expr::MethodCall(_)))
    }

    fn update_accessor_returns_owned(&self) -> bool {
        self.update_opts
            .as_ref()
            .and_then(|o| o.accessor.as_ref())
            .is_some_and(|expr| matches!(expr, syn::Expr::Call(_) | syn::Expr::MethodCall(_)))
    }

    fn parent_accessor(&self, name: &syn::Ident) -> proc_macro2::TokenStream {
        if let Some(accessor) = &self.parent_opts.as_ref().and_then(|o| o.accessor.as_ref()) {
            quote! {
                #accessor
            }
        } else {
            self.update_accessor(name)
        }
    }
}

#[derive(Default, PartialEq, FromMeta)]
struct CreateOpts {
    persist: Option<bool>,
    accessor: Option<syn::Expr>,
}

#[derive(Default, PartialEq, FromMeta)]
struct UpdateOpts {
    persist: Option<bool>,
    accessor: Option<syn::Expr>,
}

#[derive(PartialEq, Debug, Default)]
struct ListForOpts {
    by_columns: Vec<syn::Ident>,
}

impl FromMeta for ListForOpts {
    fn from_word() -> darling::Result<Self> {
        Ok(ListForOpts {
            by_columns: vec![syn::Ident::new("id", proc_macro2::Span::call_site())],
        })
    }

    fn from_bool(value: bool) -> darling::Result<Self> {
        if value {
            Self::from_word()
        } else {
            Err(darling::Error::custom(
                "list_for = false is not supported; remove list_for entirely to disable",
            ))
        }
    }

    fn from_list(items: &[darling::ast::NestedMeta]) -> darling::Result<Self> {
        let mut by_columns = Vec::new();
        for item in items {
            match item {
                darling::ast::NestedMeta::Meta(syn::Meta::List(list))
                    if list.path.is_ident("by") =>
                {
                    let inner: syn::punctuated::Punctuated<syn::Ident, syn::Token![,]> =
                        list.parse_args_with(syn::punctuated::Punctuated::parse_terminated)?;
                    by_columns.extend(inner);
                }
                _ => {
                    return Err(
                        darling::Error::custom("Expected `by(col1, col2, ...)`").with_span(item)
                    );
                }
            }
        }
        Ok(ListForOpts { by_columns })
    }
}

#[derive(PartialEq, Debug, Default)]
struct ParentOpts {
    accessor: Option<syn::Expr>,
}

impl FromMeta for ParentOpts {
    fn from_word() -> darling::Result<Self> {
        Ok(ParentOpts::default())
    }

    fn from_list(items: &[darling::ast::NestedMeta]) -> darling::Result<Self> {
        #[derive(FromMeta)]
        struct Inner {
            #[darling(default)]
            accessor: Option<syn::Expr>,
        }

        let inner = Inner::from_list(items)?;
        Ok(ParentOpts {
            accessor: inner.accessor,
        })
    }
}

#[cfg(test)]
mod tests {
    use darling::FromMeta;
    use syn::parse_quote;

    use super::*;

    #[test]
    fn column_opts_from_list() {
        let input: syn::Meta = parse_quote!(thing(
            ty = "crate::module::Thing",
            list_by = false,
            create(persist = true, accessor = accessor_fn()),
        ));
        let values = ColumnOpts::from_meta(&input).expect("Failed to parse Field");
        assert_eq!(values.ty, parse_quote!(crate::module::Thing));
        assert!(!values.list_by());
        assert!(values.find_by());
        // assert!(values.update());
        assert_eq!(
            values.create_opts.unwrap().accessor.unwrap(),
            parse_quote!(accessor_fn())
        );
    }

    #[test]
    fn columns_from_list() {
        let input: syn::Meta = parse_quote!(columns(
            name = "String",
            email(
                ty = "String",
                list_by = false,
                create(accessor = "email()"),
                update(persist = false)
            )
        ));
        let columns = Columns::from_meta(&input).expect("Failed to parse Fields");
        assert_eq!(columns.all.len(), 2);

        assert_eq!(columns.all[0].name.to_string(), "name");

        assert_eq!(columns.all[1].name.to_string(), "email");
        assert!(!columns.all[1].opts.list_by());
        assert_eq!(
            columns.all[1]
                .opts
                .create_accessor(&parse_quote!(email))
                .to_string(),
            quote!(email()).to_string()
        );
        assert!(!columns.all[1].opts.persist_on_update());
    }

    #[test]
    fn parent_opts_from_list() {
        let input: syn::Meta = parse_quote!(thing(ty = "String", parent));
        let values = ColumnOpts::from_meta(&input).expect("Failed to parse Field");
        assert_eq!(values.ty, parse_quote!(String));
        assert!(values.parent_opts.is_some());

        let input: syn::Meta = parse_quote!(thing(ty = "String", parent(accessor = "parent_id()")));
        let values = ColumnOpts::from_meta(&input).expect("Failed to parse Field");
        assert_eq!(values.ty, parse_quote!(String));
        assert!(values.parent_opts.is_some());
        assert_eq!(
            values.parent_accessor(&parse_quote!(thing)).to_string(),
            quote!(parent_id()).to_string()
        );
    }

    #[test]
    fn list_for_bare_word() {
        let input: syn::Meta = parse_quote!(thing(ty = "String", list_for));
        let values = ColumnOpts::from_meta(&input).expect("Failed to parse Field");
        assert!(values.list_for());
        assert_eq!(values.list_for_by_columns().len(), 1);
        assert_eq!(values.list_for_by_columns()[0].to_string(), "id");
    }

    #[test]
    fn list_for_with_by_columns() {
        let input: syn::Meta = parse_quote!(thing(ty = "String", list_for(by(created_at))));
        let values = ColumnOpts::from_meta(&input).expect("Failed to parse Field");
        assert!(values.list_for());
        assert_eq!(values.list_for_by_columns().len(), 1);
        assert_eq!(values.list_for_by_columns()[0].to_string(), "created_at");
    }

    #[test]
    fn list_for_by_column_must_be_list_by() {
        let id_ident: syn::Ident = parse_quote!(TestId);
        let col = Column::new_list_for(
            parse_quote!(status),
            syn::parse_str("String").unwrap(),
            vec![parse_quote!(nonexistent)],
        );
        let columns = Columns::new(&id_ident, vec![col]);
        let result = columns.validate_list_for_by_columns();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("nonexistent"),
            "error should mention the invalid column name: {err}"
        );
        assert!(
            err.contains("list_by"),
            "error should mention list_by: {err}"
        );
    }

    #[test]
    fn list_for_by_valid_column_passes_validation() {
        let id_ident: syn::Ident = parse_quote!(TestId);
        let col = Column::new_list_for(
            parse_quote!(status),
            syn::parse_str("String").unwrap(),
            vec![parse_quote!(id)],
        );
        let columns = Columns::new(&id_ident, vec![col]);
        let result = columns.validate_list_for_by_columns();
        assert!(result.is_ok());
    }

    #[test]
    fn list_for_with_multiple_by_columns() {
        let input: syn::Meta = parse_quote!(thing(ty = "String", list_for(by(created_at, id))));
        let values = ColumnOpts::from_meta(&input).expect("Failed to parse Field");
        assert!(values.list_for());
        assert_eq!(values.list_for_by_columns().len(), 2);
        assert_eq!(values.list_for_by_columns()[0].to_string(), "created_at");
        assert_eq!(values.list_for_by_columns()[1].to_string(), "id");
    }
}
