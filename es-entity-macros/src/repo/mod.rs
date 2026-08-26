mod begin;
mod combo_cursor;
mod create_all_fn;
mod create_fn;
mod delete_fn;
mod error_classifier;
mod error_types;
mod events_write;
mod find_all_fn;
mod find_by_fn;
mod forget_fn;
mod list_by_fn;
mod list_for_filters_fn;
mod list_for_fn;
mod nested;
mod options;
mod persist_events_batch_fn;
mod persist_events_fn;
mod populate_nested;
mod post_hydrate_hook;
mod post_persist_hook;
mod scope;
mod update_all_fn;
mod update_fn;

use darling::{FromDeriveInput, ToTokens};
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use options::RepositoryOptions;

pub fn derive(ast: syn::DeriveInput) -> darling::Result<proc_macro2::TokenStream> {
    let opts = RepositoryOptions::from_derive_input(&ast)?;
    opts.columns.validate_list_for_by_columns()?;
    opts.columns.validate_scope()?;
    opts.validate_forgettable()?;
    // `include_bytes!` the resolved migrations so Cargo re-runs this derive when
    // they change (keeps the migration-derived index catalog and error mapping
    // in sync even for an auto-discovered ancestor `migrations/`).
    let migrations_rerun = opts.migrations_rerun_tokens();
    let repo = EsRepo::from(&opts);
    Ok(quote!(#migrations_rerun #repo))
}
pub struct EsRepo<'a> {
    repo: &'a syn::Ident,
    generics: &'a syn::Generics,
    error_classifier: error_classifier::ErrorClassifier<'a>,
    extract_concurrent_modification_fn: Option<TokenStream>,
    persist_events_fn: Option<persist_events_fn::PersistEventsFn<'a>>,
    persist_events_batch_fn: Option<persist_events_batch_fn::PersistEventsBatchFn<'a>>,
    update_fn: update_fn::UpdateFn<'a>,
    update_all_fn: update_all_fn::UpdateAllFn<'a>,
    create_fn: create_fn::CreateFn<'a>,
    create_all_fn: create_all_fn::CreateAllFn<'a>,
    delete_fn: delete_fn::DeleteFn<'a>,
    forget_fn: Option<forget_fn::ForgetFn<'a>>,
    find_by_fns: Vec<find_by_fn::FindByFn<'a>>,
    find_all_fn: find_all_fn::FindAllFn<'a>,
    post_hydrate_hook: post_hydrate_hook::PostHydrateHook<'a>,
    post_persist_hook: post_persist_hook::PostPersistHook<'a>,
    begin: begin::Begin<'a>,
    list_by_fns: Vec<list_by_fn::ListByFn<'a>>,
    list_for_fns: Vec<list_for_fn::ListForFn<'a>>,
    nested_fns: Vec<syn::Ident>,
    nested_include_deleted_fns: Vec<syn::Ident>,
    nested: Vec<nested::Nested<'a>>,
    populate_nested: Option<populate_nested::PopulateNested<'a>>,
    error_types: error_types::ErrorTypes<'a>,
    opts: &'a RepositoryOptions,
}

impl<'a> From<&'a RepositoryOptions> for EsRepo<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        let find_by_fns = opts
            .columns
            .all_find_by()
            .map(|c| find_by_fn::FindByFn::new(c, opts))
            .collect();
        let list_by_fns = opts
            .columns
            .all_list_by()
            .map(|c| list_by_fn::ListByFn::new(c, opts))
            .collect();
        let list_for_fns = opts
            .columns
            .all_list_for()
            .flat_map(|for_col| {
                for_col
                    .list_for_by_columns()
                    .iter()
                    .filter_map(|by_name| {
                        opts.columns
                            .find_list_by(by_name)
                            .map(|by_col| list_for_fn::ListForFn::new(for_col, by_col, opts))
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        let populate_nested = opts
            .columns
            .parent()
            .map(|c| populate_nested::PopulateNested::new(c, opts));
        let nested_include_deleted_fns: Vec<_> = opts
            .all_nested()
            .map(|n| n.find_nested_include_deleted_fn_name())
            .collect();
        let (nested_fns, nested): (Vec<_>, Vec<_>) = opts
            .all_nested()
            .map(|n| (n.find_nested_fn_name(), nested::Nested::new(n, opts)))
            .unzip();

        let forget_fn = if opts.forgettable_enabled() {
            Some(forget_fn::ForgetFn::from(opts))
        } else {
            None
        };

        // Every write path now folds its index write and its event insert into
        // one statement, so the shared persist helpers are only reachable
        // where there is no index write to fold into. Emitting a helper
        // nothing calls would be dead code in consumers that build with
        // `-D warnings`, so each is gated to its remaining callers:
        // `persist_events` for column-less updates and for `forget` on repos
        // with no forgettable index columns; `persist_events_batch` for
        // column-less bulk updates. `extract_concurrent_modification` outlives
        // both — the follow-up forgettable payload inserts still use it.
        let needs_persist_events = !opts.columns.updates_needed()
            || (opts.forgettable_enabled() && opts.columns.forgettable_column_names().is_empty());
        let needs_persist_events_batch = !opts.columns.updates_needed();
        let extract_concurrent_modification_fn = (!opts.columns.updates_needed()
            || opts.forgettable_enabled())
        .then(error_classifier::extract_concurrent_modification_fn);

        Self {
            repo: &opts.ident,
            generics: &opts.generics,
            error_classifier: error_classifier::ErrorClassifier::from(opts),
            extract_concurrent_modification_fn,
            persist_events_fn: needs_persist_events
                .then(|| persist_events_fn::PersistEventsFn::from(opts)),
            persist_events_batch_fn: needs_persist_events_batch
                .then(|| persist_events_batch_fn::PersistEventsBatchFn::from(opts)),
            update_fn: update_fn::UpdateFn::from(opts),
            update_all_fn: update_all_fn::UpdateAllFn::from(opts),
            create_fn: create_fn::CreateFn::from(opts),
            create_all_fn: create_all_fn::CreateAllFn::from(opts),
            delete_fn: delete_fn::DeleteFn::from(opts),
            forget_fn,
            find_by_fns,
            find_all_fn: find_all_fn::FindAllFn::from(opts),
            post_hydrate_hook: post_hydrate_hook::PostHydrateHook::from(opts),
            post_persist_hook: post_persist_hook::PostPersistHook::from(opts),
            begin: begin::Begin::from(opts),
            list_by_fns,
            list_for_fns,
            nested_fns,
            nested_include_deleted_fns,
            nested,
            populate_nested,
            error_types: error_types::ErrorTypes::new(opts),
            opts,
        }
    }
}

impl ToTokens for EsRepo<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let repo = &self.repo;
        let error_classifier = &self.error_classifier;
        let extract_concurrent_modification_fn = &self.extract_concurrent_modification_fn;
        let persist_events_fn = &self.persist_events_fn;
        let persist_events_batch_fn = &self.persist_events_batch_fn;
        let update_fn = &self.update_fn;
        let update_all_fn = &self.update_all_fn;
        let create_fn = &self.create_fn;
        let create_all_fn = &self.create_all_fn;
        let delete_fn = &self.delete_fn;
        let forget_fn = &self.forget_fn;
        let find_by_fns = &self.find_by_fns;
        let find_all_fn = &self.find_all_fn;
        let post_hydrate_hook = &self.post_hydrate_hook;
        let post_persist_hook = &self.post_persist_hook;
        let begin = &self.begin;
        let cursors = self.list_by_fns.iter().map(|l| l.cursor());
        let combo_cursor = combo_cursor::ComboCursor::new(
            self.opts,
            self.list_by_fns.iter().map(|l| l.cursor()).collect(),
        );
        let sort_by = combo_cursor.sort_by();
        let list_for_filters = list_for_filters_fn::ListForFiltersFn::new(
            self.opts,
            self.opts.columns.all_list_for().collect(),
            self.opts.columns.all_list_by().collect(),
            &combo_cursor,
        );
        let list_for_filters_struct = &list_for_filters.filters_struct;
        #[cfg(feature = "graphql")]
        let gql_combo_cursor = combo_cursor.gql_cursor();
        #[cfg(not(feature = "graphql"))]
        let gql_combo_cursor = TokenStream::new();
        #[cfg(feature = "graphql")]
        let gql_cursors: Vec<_> = self
            .list_by_fns
            .iter()
            .map(|l| l.cursor().gql_cursor())
            .collect();
        #[cfg(not(feature = "graphql"))]
        let gql_cursors: Vec<TokenStream> = Vec::new();
        let list_by_fns = &self.list_by_fns;
        let list_for_fns = &self.list_for_fns;

        let entity = self.opts.entity();
        let event = self.opts.event();
        let id = self.opts.id();

        let cursor_mod = self.opts.cursor_mod();
        let types_mod = self.opts.repo_types_mod();

        let nested_fns = &self.nested_fns;
        let nested_include_deleted_fns = &self.nested_include_deleted_fns;
        let nested = &self.nested;
        let populate_nested = &self.populate_nested;

        let pool_field = self.opts.pool_field();
        let has_tbl_prefix = self.opts.table_prefix().is_some();
        let es_query_flavor = if nested_fns.is_empty() {
            quote! {
                es_entity::EsQueryFlavorFlat
            }
        } else {
            quote! { es_entity::EsQueryFlavorNested }
        };

        let create_error = self.opts.create_error();
        let modify_error = self.opts.modify_error();
        let find_error = self.opts.find_error();
        let query_error = self.opts.query_error();
        let error_types = self.error_types.generate();
        let map_constraint_fn = self.error_types.generate_map_constraint_fn();

        let scope_type = scope::ScopeType::new(self.opts);
        let scope_type = quote! { #scope_type };

        let (impl_generics, ty_generics, where_clause) = self.generics.split_for_impl();

        // The `repo.scoped(scope)` bound view: a borrowed view of the repo
        // with the scope captured once, exposing every read fn without the
        // per-call scope argument (each method delegates to the scope-arg
        // fn). Only generated for scoped repos.
        let (scoped_fn, scoped_view) = if let Some(info) = scope::ScopeInfo::from_opts(self.opts) {
            let scoped_ident = self.opts.scoped_view_ident();
            let scope_ty = &info.scope_ty;
            let repo_ident = self.repo;

            let mut scoped_generics = self.generics.clone();
            scoped_generics
                .params
                .insert(0, syn::parse_quote!('scoped_repo));
            let scoped_struct_where = scoped_generics.where_clause.clone();
            let (scoped_impl_generics, scoped_ty_generics, scoped_where) =
                scoped_generics.split_for_impl();

            let find_by_delegates = self.find_by_fns.iter().map(|f| f.scoped_delegates());
            let find_all_delegates = self.find_all_fn.scoped_delegates();
            let list_by_delegates = self.list_by_fns.iter().map(|f| f.scoped_delegates());
            let list_for_delegates = self.list_for_fns.iter().map(|f| f.scoped_delegates());
            let list_for_filters_delegates = list_for_filters.scoped_delegates();

            let scoped_doc = format!(
                "Bound view of [`{repo_ident}`] with a [`{scope_ty}`] captured once: every \
                 read method delegates to the corresponding scope-argument fn with the bound \
                 scope. Obtained via [`{repo_ident}::scoped`]. Borrows the repo, so it is \
                 naturally request-scoped."
            );

            (
                quote! {
                    pub fn scoped<'scoped_repo>(
                        &'scoped_repo self,
                        scope: impl Into<#scope_ty>,
                    ) -> #scoped_ident #scoped_ty_generics {
                        #scoped_ident {
                            repo: self,
                            scope: scope.into(),
                        }
                    }
                },
                quote! {
                    #[doc = #scoped_doc]
                    pub struct #scoped_ident #scoped_generics #scoped_struct_where {
                        repo: &'scoped_repo #repo_ident #ty_generics,
                        scope: #scope_ty,
                    }

                    impl #scoped_impl_generics #scoped_ident #scoped_ty_generics #scoped_where {
                        #[inline(always)]
                        pub fn scope(&self) -> #scope_ty {
                            self.scope
                        }

                        #(#find_by_delegates)*
                        #find_all_delegates
                        #(#list_by_delegates)*
                        #(#list_for_delegates)*
                        #list_for_filters_delegates
                    }
                },
            )
        } else {
            (quote! {}, quote! {})
        };

        // If the event type has Forgettable fields, the repo must enable
        // `forgettable` — otherwise the payload machinery is never generated
        // and forgettable values would be lost. The repo cannot see the
        // event's forgettable-ness at macro time, so this rides the event's
        // inherent `HAS_FORGETTABLE_FIELDS` const as a const assert (mirroring
        // the `es_query!` guard). Forgettable *index columns* are checked
        // eagerly in `validate_forgettable`.
        let forgettable_event_guard = if self.opts.forgettable_enabled() {
            quote! {}
        } else {
            quote! {
                const _: () = assert!(
                    !Repo__Event::HAS_FORGETTABLE_FIELDS,
                    "event type has Forgettable fields but this repo does not enable `forgettable`; add `forgettable` to #[es_repo(...)]"
                );
            }
        };

        tokens.append_all(quote! {
            pub mod #cursor_mod {
                use super::*;

                #(#cursors)*
                #(#gql_cursors)*

                #combo_cursor
                #gql_combo_cursor
            }

            mod #types_mod {

                use super::*;

                #[allow(non_camel_case_types)]
                pub(super) type Repo__Id = #id;
                #[allow(non_camel_case_types)]
                pub(super) type Repo__Event = #event;
                #[allow(non_camel_case_types)]
                pub(super) type Repo__Entity = #entity;
                #[allow(non_camel_case_types)]
                pub(super) type Repo__DbEvent = es_entity::GenericEvent<#id>;
                #[allow(dead_code)]
                pub(super) const REPO__HAS_TBL_PREFIX: bool = #has_tbl_prefix;

                #forgettable_event_guard
            }

            #error_types

            #scope_type

            #scoped_view

            #list_for_filters_struct
            #sort_by

             impl #impl_generics #repo #ty_generics #where_clause {
                #[inline(always)]
                pub fn pool(&self) -> &es_entity::db::Pool {
                    &self.#pool_field
                }

                #scoped_fn

                #map_constraint_fn
                #error_classifier
                #begin
                #post_hydrate_hook
                #post_persist_hook
                #extract_concurrent_modification_fn
                #persist_events_fn
                #persist_events_batch_fn
                #create_fn
                #create_all_fn
                #update_fn
                #update_all_fn
                #delete_fn
                #forget_fn
                #(#find_by_fns)*
                #find_all_fn
                #list_for_filters
                #(#list_by_fns)*
                #(#list_for_fns)*
                #(#nested)*
            }

            #populate_nested

            impl #impl_generics es_entity::EsRepo for #repo #ty_generics #where_clause {
                type Entity = #entity;
                type CreateError = #create_error;
                type ModifyError = #modify_error;
                type FindError = #find_error;
                type QueryError = #query_error;
                type EsQueryFlavor = #es_query_flavor;

               #[inline(always)]
               async fn load_all_nested_in_op<OP, __EsErr>(
                   op: &mut OP, entities: &mut [#entity]
               ) -> Result<(), __EsErr>
                   where
                       OP: es_entity::AtomicOperation,
                       __EsErr: From<sqlx::Error> + From<es_entity::EntityHydrationError> + Send,
               {
                   #(Self::#nested_fns::<_, _, __EsErr>(op, entities).await?;)*
                   Ok(())
               }

               #[inline(always)]
               async fn load_all_nested_in_op_include_deleted<OP, __EsErr>(
                   op: &mut OP, entities: &mut [#entity]
               ) -> Result<(), __EsErr>
                   where
                       OP: es_entity::AtomicOperation,
                       __EsErr: From<sqlx::Error> + From<es_entity::EntityHydrationError> + Send,
               {
                   #(Self::#nested_include_deleted_fns::<_, _, __EsErr>(op, entities).await?;)*
                   Ok(())
               }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    // Guard 2 (known at macro time): Forgettable<T> index columns require the
    // repo to enable `forgettable`.
    #[test]
    fn forgettable_index_column_without_flag_is_error() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(entity = "Subscriber", columns(email(ty = "Forgettable<String>")))]
            struct Subscribers {
                pool: sqlx::PgPool,
            }
        };
        let err = derive(input).unwrap_err();
        assert!(
            err.to_string().contains("does not enable `forgettable`"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn forgettable_index_column_with_flag_is_ok() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(
                entity = "Subscriber",
                forgettable,
                columns(email(ty = "Forgettable<String>"))
            )]
            struct Subscribers {
                pool: sqlx::PgPool,
            }
        };
        assert!(derive(input).is_ok());
    }

    /// The combined-write classifiers are emitted once per repo, not rendered
    /// into every write path. `classify_write_error` is gated to repos that
    /// actually have a caller — an uncalled private helper is dead code, and
    /// consumers build with `-D warnings`.
    #[test]
    fn error_classifiers_are_emitted_once_and_gated() {
        let with_columns: syn::DeriveInput = parse_quote! {
            #[es_repo(entity = "User", columns(name(ty = "String")))]
            struct Users {
                pool: sqlx::PgPool,
            }
        };
        let out = derive(with_columns).unwrap().to_string();
        assert_eq!(
            out.matches("fn classify_create_error").count(),
            1,
            "create classifier should be defined exactly once"
        );
        assert_eq!(
            out.matches("fn classify_write_error").count(),
            1,
            "write classifier should be defined exactly once"
        );
        // Both create paths call the shared fn rather than inlining a match.
        assert_eq!(
            out.matches("Self :: classify_create_error").count(),
            2,
            "create and create_all should both call the shared classifier"
        );

        // No index columns to persist and no soft delete → nothing calls the
        // write classifier, so it must not be emitted.
        let no_columns: syn::DeriveInput = parse_quote! {
            #[es_repo(entity = "User")]
            struct Users {
                pool: sqlx::PgPool,
            }
        };
        let out = derive(no_columns).unwrap().to_string();
        assert!(
            !out.contains("fn classify_write_error"),
            "write classifier must not be emitted without a caller"
        );
        assert!(
            out.contains("fn classify_create_error"),
            "create classifier always has callers"
        );
    }

    // Guard 1 (event has Forgettable fields but the repo omits `forgettable`)
    // fires only once the event type resolves, so it is a const assert on the
    // event's inherent `HAS_FORGETTABLE_FIELDS`; its end-to-end behavior is
    // covered by a compile_fail doctest on `Forgettable` rather than a brittle
    // token-string assertion here.

    #[test]
    fn plain_repo_is_ok() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(entity = "User", columns(name(ty = "String")))]
            struct Users {
                pool: sqlx::PgPool,
            }
        };
        assert!(derive(input).is_ok());
    }

    #[test]
    fn scoped_repo_is_ok() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(
                entity = "User",
                columns(partner_id(ty = "PartnerId", scope), name(ty = "String"))
            )]
            struct Users {
                pool: sqlx::PgPool,
            }
        };
        let tokens = derive(input)
            .expect("scoped repo should derive")
            .to_string();
        assert!(tokens.contains("pub enum UserScope"));
        assert!(tokens.contains("PartnerId (PartnerId)"));
        assert!(!tokens.contains("Only"));
        // every read fn takes the scope argument
        assert!(tokens.contains("fn find_by_id (& self , scope : impl Into < UserScope >"));
        assert!(tokens.contains("(& self , scope : impl Into < UserScope > , ids"));
        assert!(tokens.contains("fn list_by_created_at (& self , scope : impl Into < UserScope >"));
        // the PartnerId arm filters by the scope column, the All arm does not
        assert!(tokens.contains("WHERE id = $1 AND partner_id = $2"));
        assert!(tokens.contains("WHERE id = $1\""));
        // no find_by fns are generated for the scope column itself
        assert!(!tokens.contains("find_by_partner_id"));
        // writes stay unscoped (custody principle)
        assert!(tokens.contains("fn create_in_op < OP > (& self , op : & mut OP , new_entity"));
        assert!(tokens.contains("fn update_in_op < OP > (& self , op : & mut OP , entity"));
    }

    #[test]
    fn scoped_repo_generates_bound_view() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(
                entity = "User",
                columns(partner_id(ty = "PartnerId", scope), name(ty = "String"))
            )]
            struct Users {
                pool: sqlx::PgPool,
            }
        };
        let tokens = derive(input)
            .expect("scoped repo should derive")
            .to_string();
        // the bound view type + constructor
        assert!(tokens.contains("pub struct ScopedUsers"));
        assert!(tokens.contains("pub fn scoped <"));
        // view methods take no scope argument and forward the bound scope
        assert!(tokens.contains("self . repo . find_by_id (self . scope"));
        assert!(tokens.contains("self . repo . maybe_find_by_id_in_op (op , self . scope"));
        assert!(tokens.contains("self . repo . find_all (self . scope"));
        assert!(tokens.contains("self . repo . list_by_created_at (self . scope"));
        assert!(tokens.contains("self . repo . list_for_filters (self . scope"));
    }

    #[test]
    fn unscoped_repo_has_no_bound_view() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(entity = "User", columns(name(ty = "String")))]
            struct Users {
                pool: sqlx::PgPool,
            }
        };
        let tokens = derive(input).unwrap().to_string();
        assert!(!tokens.contains("ScopedUsers"));
        assert!(!tokens.contains("pub fn scoped <"));
    }

    #[test]
    fn scoped_repo_with_generics_generates_bound_view() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(
                entity = "User",
                columns(partner_id(ty = "PartnerId", scope))
            )]
            struct Users<E> {
                pool: sqlx::PgPool,
                _phantom: std::marker::PhantomData<E>,
            }
        };
        let tokens = derive(input)
            .expect("generic scoped repo should derive")
            .to_string();
        assert!(tokens.contains("pub struct ScopedUsers < 'scoped_repo , E >"));
    }

    #[test]
    fn multi_scoped_repo_is_ok() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(
                entity = "Facility",
                columns(
                    partner_id(ty = "PartnerId", scope),
                    customer_id(ty = "CustomerId", scope),
                    name(ty = "String")
                )
            )]
            struct Facilities {
                pool: sqlx::PgPool,
            }
        };
        let tokens = derive(input)
            .expect("multi-scoped repo should derive")
            .to_string();
        assert!(tokens.contains("pub enum FacilityScope"));
        assert!(tokens.contains("PartnerId (PartnerId)"));
        assert!(tokens.contains("CustomerId (CustomerId)"));
        // one dispatch arm per dimension, each with only its own conjunct
        assert!(tokens.contains("WHERE id = $1 AND partner_id = $2"));
        assert!(tokens.contains("WHERE id = $1 AND customer_id = $2"));
        assert!(!tokens.contains("partner_id = $2 AND customer_id"));
        // From impls for both id types
        assert!(tokens.contains("impl From < PartnerId > for FacilityScope"));
        assert!(tokens.contains("impl From < CustomerId > for FacilityScope"));
    }

    #[test]
    fn scope_variant_override_is_ok() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(
                entity = "Facility",
                columns(
                    partner_id(ty = "PartnerId", scope(variant = "Partner")),
                    customer_id(ty = "CustomerId", scope),
                    name(ty = "String")
                )
            )]
            struct Facilities {
                pool: sqlx::PgPool,
            }
        };
        let tokens = derive(input)
            .expect("overridden-variant scoped repo should derive")
            .to_string();
        assert!(tokens.contains("pub enum FacilityScope"));
        // overridden column uses the custom variant name...
        assert!(tokens.contains("Partner (PartnerId)"));
        assert!(!tokens.contains("PartnerId (PartnerId)"));
        // ...while the un-overridden column keeps the default
        assert!(tokens.contains("CustomerId (CustomerId)"));
        assert!(tokens.contains("impl From < PartnerId > for FacilityScope"));
        assert!(tokens.contains("FacilityScope :: Partner (__scope_val)"));
    }

    #[test]
    fn id_scope_variant_override_is_ok() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(
                entity = "Partner",
                columns(id(scope(variant = "Tenant")), name(ty = "String", list_by))
            )]
            struct Partners {
                pool: sqlx::PgPool,
            }
        };
        let tokens = derive(input)
            .expect("id(scope(variant = ...)) repo should derive")
            .to_string();
        assert!(tokens.contains("pub enum PartnerScope"));
        // the id column's overridden variant name replaces the default `Id`...
        assert!(tokens.contains("Tenant (PartnerId)"));
        assert!(!tokens.contains("Id (PartnerId)"));
        assert!(tokens.contains("impl From < PartnerId > for PartnerScope"));
        assert!(tokens.contains("PartnerScope :: Tenant (__scope_val)"));
        // every query carries the id conjunct under the overridden variant
        assert!(tokens.contains("WHERE id = $1 AND id = $2"));
    }

    #[test]
    fn same_type_scope_columns_is_error() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(
                entity = "User",
                columns(
                    partner_id(ty = "PartnerId", scope),
                    other_partner_id(ty = "PartnerId", scope)
                )
            )]
            struct Users {
                pool: sqlx::PgPool,
            }
        };
        let err = derive(input).unwrap_err();
        assert!(
            err.to_string().contains("same Rust type"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn optional_scope_column_is_ok() {
        // A syntactic Option<T> scope column is accepted: the generated
        // variant and its `From` impl carry the bare inner type, never
        // `Option<T>` — a scope value is always concrete, and NULL rows are
        // simply invisible to this variant (only `All` sees them).
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(
                entity = "User",
                columns(partner_id(ty = "Option<PartnerId>", scope))
            )]
            struct Users {
                pool: sqlx::PgPool,
            }
        };
        let tokens = derive(input)
            .expect("Option<T> scope column should derive")
            .to_string();
        assert!(tokens.contains("pub enum UserScope"));
        // the variant carries the bare inner type, not Option<PartnerId>.
        // The full derive output legitimately still contains "Option <
        // PartnerId >" elsewhere (the column itself stays declared
        // `Option<PartnerId>` for create/persist — only the scope enum and
        // its `From` impls unwrap it), so assert on the specific
        // scope-related spots rather than the whole token stream.
        assert!(tokens.contains("PartnerId (PartnerId)"));
        assert!(!tokens.contains("PartnerId (Option < PartnerId >)"));
        assert!(tokens.contains("impl From < PartnerId > for UserScope"));
        assert!(tokens.contains("impl From < & PartnerId > for UserScope"));
        assert!(!tokens.contains("impl From < Option < PartnerId > > for UserScope"));
        // the SQL conjunct is the ordinary equality — NULL rows fall out of
        // `=` semantics with zero special-casing.
        assert!(tokens.contains("WHERE id = $1 AND partner_id = $2"));
    }

    #[test]
    fn nullable_annotated_scope_column_is_error() {
        // Unlike Option<T>, a `nullable`-annotated non-Option scope column
        // has no inner type to unwrap — still rejected.
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(
                entity = "User",
                columns(partner_id(ty = "PartnerId", scope, nullable = true))
            )]
            struct Users {
                pool: sqlx::PgPool,
            }
        };
        let err = derive(input).unwrap_err();
        assert!(
            err.to_string().contains("only syntactic Option<T>"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn scope_column_composes_with_query_flags() {
        // `scope` may coexist with `find_by = true`, `list_by` and `list_for`
        // on the same column — the generated fns compose with the scope.
        for extra in ["find_by = true", "list_by = true", "list_for"] {
            let src = format!(
                r#"
                #[es_repo(
                    entity = "User",
                    columns(partner_id(ty = "PartnerId", scope, {extra}))
                )]
                struct Users {{
                    pool: sqlx::PgPool,
                }}
                "#
            );
            let input: syn::DeriveInput = syn::parse_str(&src).unwrap();
            derive(input)
                .unwrap_or_else(|err| panic!("scope column with `{extra}` should derive: {err}"));
        }
    }

    #[test]
    fn scope_column_find_by_composes_via_conjunct() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(
                entity = "User",
                columns(partner_id(ty = "PartnerId", scope, find_by = true))
            )]
            struct Users {
                pool: sqlx::PgPool,
            }
        };
        let tokens = derive(input)
            .expect("scoped repo should derive")
            .to_string();
        // explicit opt-in generates the find fns, scope argument included
        assert!(tokens.contains("fn find_by_partner_id (& self , scope : impl Into < UserScope >"));
        assert!(
            tokens
                .contains("fn maybe_find_by_partner_id (& self , scope : impl Into < UserScope >")
        );
        assert!(tokens.contains("WHERE partner_id = $1 AND partner_id = $2"));
    }

    #[test]
    fn scope_column_list_for_composes_via_conjunct() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(
                entity = "User",
                columns(
                    partner_id(ty = "PartnerId", scope, list_for(by(created_at))),
                    status(ty = "String", list_for(by(created_at)))
                )
            )]
            struct Users {
                pool: sqlx::PgPool,
            }
        };
        let tokens = derive(input)
            .expect("scoped repo should derive")
            .to_string();
        // the Filters struct carries the scope column like any other
        // list_for column
        assert!(tokens.contains("pub struct UserFilters"));
        assert!(tokens.contains("pub partner_id : Option < PartnerId >"));
        assert!(tokens.contains("pub status : Option < String >"));
        assert!(tokens.contains(
            "fn list_for_partner_id_by_created_at (& self , scope : impl Into < UserScope >"
        ));
        assert!(tokens.contains("(partner_id = $1) AND partner_id = $2"));
    }

    #[test]
    fn scope_on_nested_child_repo_is_error() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(
                entity = "LineItem",
                columns(
                    order_id(ty = "OrderId", parent),
                    partner_id(ty = "PartnerId", scope)
                )
            )]
            struct LineItems {
                pool: sqlx::PgPool,
            }
        };
        let err = derive(input).unwrap_err();
        assert!(
            err.to_string().contains("not supported on nested repos"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn forgettable_scope_column_is_error() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(
                entity = "User",
                forgettable,
                columns(partner_id(ty = "Forgettable<PartnerId>", scope))
            )]
            struct Users {
                pool: sqlx::PgPool,
            }
        };
        let err = derive(input).unwrap_err();
        // Forgettable columns are rewritten to Option<T>, so either the
        // forgettable or the nullable check may fire first — both reject.
        let msg = err.to_string();
        assert!(
            msg.contains("Forgettable") || msg.contains("non-nullable"),
            "unexpected error: {msg}"
        );
    }
}
