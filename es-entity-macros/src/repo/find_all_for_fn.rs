use darling::ToTokens;
use proc_macro2::{Span, TokenStream};
use quote::{TokenStreamExt, quote};

use super::{
    list_by_fn::{CursorStruct, not_deleted_predicate},
    options::*,
    scope::{ScopeCol, ScopeInfo},
};

/// `find_all_for_{col}s`: the batch lookup keyed by a `list_for` column.
pub struct FindAllForFn<'a> {
    in_op_only: bool,
    prefix: Option<&'a syn::LitStr>,
    for_column: &'a Column,
    by_column: &'a Column,
    entity: &'a syn::Ident,
    id: &'a syn::Ident,
    table_name: &'a str,
    delete: DeleteOption,
    cursor_mod: syn::Ident,
    any_nested: bool,
    post_hydrate_hook: bool,
    forgettable_table_name: Option<&'a str>,
    snapshot_table_name: Option<&'a str>,
    scope: Option<ScopeInfo<'a>>,
    #[cfg(feature = "instrument")]
    repo_name_snake: String,
}

impl<'a> FindAllForFn<'a> {
    /// `None` unless `for_column` opted in with `find_all_for` and has a
    /// resolvable first `by(..)` column. `validate_find_all_for` rejects an
    /// opted-in column that cannot back the lookup before this runs.
    pub fn new(for_column: &'a Column, opts: &'a RepositoryOptions) -> Option<Self> {
        if !for_column.find_all_for_enabled() {
            return None;
        }
        let by_column = for_column
            .list_for_by_columns()
            .first()
            .and_then(|by_name| opts.columns.find_list_by(by_name))?;
        Some(Self {
            in_op_only: opts.in_op_only(),
            prefix: opts.table_prefix(),
            for_column,
            by_column,
            id: opts.id(),
            entity: opts.entity(),
            table_name: opts.table_name(),
            delete: opts.delete,
            cursor_mod: opts.cursor_mod(),
            any_nested: opts.any_nested(),
            post_hydrate_hook: opts.post_hydrate_hook.is_some(),
            forgettable_table_name: opts.forgettable_table_name(),
            snapshot_table_name: opts.snapshot_table_name(),
            scope: ScopeInfo::from_opts(opts),
            #[cfg(feature = "instrument")]
            repo_name_snake: opts.repo_name_snake_case(),
        })
    }

    fn cursor(&self) -> CursorStruct<'_> {
        CursorStruct {
            column: self.by_column,
            id: self.id,
            entity: self.entity,
            cursor_mod: &self.cursor_mod,
        }
    }

    fn plural_name(&self) -> String {
        pluralizer::pluralize(&self.for_column.name().to_string(), 2, false)
    }

    fn fn_idents(&self, delete: DeleteOption) -> (syn::Ident, syn::Ident) {
        let base = format!(
            "find_all_for_{}{}",
            self.plural_name(),
            delete.include_deletion_fn_postfix()
        );
        (
            syn::Ident::new(&base, Span::call_site()),
            syn::Ident::new(&format!("{base}_in_op"), Span::call_site()),
        )
    }

    fn deletes(&self) -> Vec<DeleteOption> {
        let mut deletes = Vec::new();
        for delete in [DeleteOption::No, DeleteOption::Soft] {
            deletes.push(delete);
            if delete == self.delete || self.delete == DeleteOption::SoftWithoutQueries {
                break;
            }
        }
        deletes
    }

    /// Delegating methods for the generated `Scoped{Repo}` bound view.
    /// Empty for unscoped repos.
    pub fn scoped_delegates(&self) -> TokenStream {
        let mut tokens = TokenStream::new();
        if self.scope.is_none() {
            return tokens;
        }
        let entity = self.entity;
        let query_fn_op_traits = RepositoryOptions::query_fn_op_traits();
        let keys_arg = syn::Ident::new(&self.plural_name(), Span::call_site());
        let key_ty = self.for_column.ty();

        for delete in self.deletes() {
            let (fn_name, fn_in_op) = self.fn_idents(delete);

            let standalone = (!self.in_op_only).then(|| {
                quote! {
                    pub async fn #fn_name<Out: From<#entity>>(
                        &self,
                        #keys_arg: &[#key_ty],
                        direction: es_entity::ListDirection,
                    ) -> Result<std::collections::HashMap<#key_ty, Vec<Out>>, es_entity::RepoFault> {
                        self.repo.#fn_name(self.scope, #keys_arg, direction).await
                    }
                }
            });

            tokens.append_all(quote! {
                #standalone

                pub async fn #fn_in_op<'a, Out: From<#entity>>(
                    &self,
                    op: impl #query_fn_op_traits,
                    #keys_arg: &[#key_ty],
                    direction: es_entity::ListDirection,
                ) -> Result<std::collections::HashMap<#key_ty, Vec<Out>>, es_entity::RepoFault> {
                    self.repo.#fn_in_op(op, self.scope, #keys_arg, direction).await
                }
            });
        }
        tokens
    }
}

impl ToTokens for FindAllForFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let entity = self.entity;
        let query_fn_op_traits = RepositoryOptions::query_fn_op_traits();
        let query_fn_get_op = RepositoryOptions::query_fn_get_op();

        let for_column_name = self.for_column.name();
        let keys_arg = syn::Ident::new(&self.plural_name(), Span::call_site());
        let key_ty = self.for_column.ty();
        let key_accessor = self.for_column.find_all_for_key_accessor();

        let cursor = self.cursor();
        let select_columns = cursor.select_columns(Some(for_column_name));

        let forgettable_tbl_arg = if let Some(tbl) = self.forgettable_table_name {
            quote! { forgettable_tbl = #tbl, }
        } else {
            quote! {}
        };
        let snapshot_tbl_arg = if let Some(tbl) = self.snapshot_table_name {
            quote! { snapshot_tbl = #tbl, }
        } else {
            quote! {}
        };

        let make_es_query = |query: &str, extra_args: &TokenStream| -> TokenStream {
            if let Some(prefix) = self.prefix {
                quote! {
                    es_entity::es_query!(
                        tbl_prefix = #prefix,
                        #forgettable_tbl_arg
                        #snapshot_tbl_arg
                        #query,
                        #keys_arg as &[#key_ty],
                        #extra_args
                    )
                }
            } else {
                quote! {
                    es_entity::es_query!(
                        entity = #entity,
                        #forgettable_tbl_arg
                        #snapshot_tbl_arg
                        #query,
                        #keys_arg as &[#key_ty],
                        #extra_args
                    )
                }
            }
        };

        let (scope_fn_arg, scope_fn_pass, scope_convert) = match &self.scope {
            Some(scope) => (scope.fn_arg(), scope.fn_pass(), scope.convert()),
            None => (quote! {}, quote! {}, quote! {}),
        };

        for delete in self.deletes() {
            let (fn_name, fn_in_op) = self.fn_idents(delete);

            let build_query_arms = |scope: Option<&ScopeCol>| -> TokenStream {
                let mut query_arms = TokenStream::new();
                for ascending in [true, false] {
                    let mut conditions = vec![format!("{for_column_name} = ANY($1)")];
                    if let Some(scope) = scope {
                        conditions.push(scope.predicate(2));
                    }
                    if delete == DeleteOption::No
                        && let Some(not_deleted) = not_deleted_predicate(self.delete)
                    {
                        conditions.push(not_deleted);
                    }
                    let order_by = if self.by_column.name() == for_column_name {
                        cursor.order_by(ascending)
                    } else {
                        format!("{for_column_name}, {}", cursor.order_by(ascending))
                    };
                    let query = format!(
                        "SELECT {select_columns} FROM {} WHERE {} ORDER BY {order_by}",
                        self.table_name,
                        conditions.join(" AND "),
                    );
                    let scope_args = scope.map(|s| s.arg_tokens()).unwrap_or_default();
                    let es_query_call = make_es_query(&query, &scope_args);
                    let direction_pattern = if ascending {
                        quote! { es_entity::ListDirection::Ascending }
                    } else {
                        quote! { es_entity::ListDirection::Descending }
                    };
                    let fetch_method = if delete == DeleteOption::Soft && self.any_nested {
                        quote! { fetch_n_include_deleted }
                    } else {
                        quote! { fetch_n }
                    };
                    query_arms.append_all(quote! {
                        #direction_pattern => {
                            #es_query_call.#fetch_method(op, usize::MAX).await?
                        },
                    });
                }
                query_arms
            };

            let match_expr = if let Some(scope) = &self.scope {
                scope.dispatch(
                    {
                        let query_arms = build_query_arms(None);
                        quote! { match direction { #query_arms } }
                    },
                    |col| {
                        let scoped_query_arms = build_query_arms(Some(col));
                        quote! { match direction { #scoped_query_arms } }
                    },
                )
            } else {
                let query_arms = build_query_arms(None);
                quote! { match direction { #query_arms } }
            };

            #[cfg(feature = "instrument")]
            let instrument_attr = {
                let entity_name = entity.to_string();
                let span_name = format!(
                    "{}.find_all_for_{}{}",
                    self.repo_name_snake,
                    self.plural_name(),
                    delete.include_deletion_fn_postfix()
                );
                quote! {
                    #[tracing::instrument(name = #span_name, skip_all, fields(entity = #entity_name, count = #keys_arg.len(), direction = tracing::field::debug(&direction)), err)]
                }
            };
            #[cfg(not(feature = "instrument"))]
            let instrument_attr = quote! {};

            let post_hydrate_check = if self.post_hydrate_hook {
                quote! {
                    for __entity in &__entities {
                        self.execute_post_hydrate_hook(__entity)?;
                    }
                }
            } else {
                quote! {}
            };

            let standalone = (!self.in_op_only).then(|| {
                quote! {
                    pub async fn #fn_name<Out: From<#entity>>(
                        &self,
                        #scope_fn_arg
                        #keys_arg: &[#key_ty],
                        direction: es_entity::ListDirection,
                    ) -> Result<std::collections::HashMap<#key_ty, Vec<Out>>, es_entity::RepoFault> {
                        self.#fn_in_op(#query_fn_get_op, #scope_fn_pass #keys_arg, direction).await
                    }
                }
            });

            tokens.append_all(quote! {
                #standalone

                #instrument_attr
                pub async fn #fn_in_op<'a, Out: From<#entity>>(
                    &self,
                    op: impl #query_fn_op_traits,
                    #scope_fn_arg
                    #keys_arg: &[#key_ty],
                    direction: es_entity::ListDirection,
                ) -> Result<std::collections::HashMap<#key_ty, Vec<Out>>, es_entity::RepoFault> {
                    #scope_convert
                    let (__entities, _) = #match_expr;
                    #post_hydrate_check
                    let mut __grouped: std::collections::HashMap<#key_ty, Vec<Out>> =
                        std::collections::HashMap::new();
                    for __entity in __entities {
                        let __key = __entity.#key_accessor.clone();
                        __grouped.entry(__key).or_default().push(Out::from(__entity));
                    }
                    Ok(__grouped)
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::super::derive;

    fn tokens(input: syn::DeriveInput) -> String {
        derive(input).expect("repo should derive").to_string()
    }

    fn deposit_accounts(extra_opts: proc_macro2::TokenStream) -> syn::DeriveInput {
        parse_quote! {
            #[es_repo(
                entity = "DepositAccount",
                #extra_opts
                columns(
                    account_holder_id(
                        ty = "AccountHolderId",
                        list_for(by(created_at, id), find_all_for)
                    ),
                    currency(ty = "Currency", list_for(by(id), find_all_for)),
                    nickname(ty = "Option<String>", list_for),
                )
            )]
            struct DepositAccounts {
                pool: sqlx::PgPool,
            }
        }
    }

    #[test]
    fn names_are_pluralised_per_for_column() {
        let tokens = tokens(deposit_accounts(quote::quote! {}));

        assert!(tokens.contains("pub async fn find_all_for_account_holder_ids <"));
        assert!(tokens.contains("pub async fn find_all_for_account_holder_ids_in_op <"));
        assert!(tokens.contains("pub async fn find_all_for_currencies <"));
        assert!(tokens.contains("pub async fn find_all_for_currencies_in_op <"));
        assert!(!tokens.contains("find_all_for_currencys"));
        assert!(!tokens.contains("find_all_for_account_holder_ids_by_"));
    }

    #[test]
    fn optional_column_emits_nothing() {
        let tokens = tokens(deposit_accounts(quote::quote! {}));

        assert!(!tokens.contains("find_all_for_nicknames"));
        assert!(!tokens.contains("find_all_for_nickname"));
    }

    #[test]
    fn query_uses_any_and_orders_by_key_then_first_by_column() {
        let tokens = tokens(deposit_accounts(quote::quote! {}));

        assert!(tokens.contains(
            "SELECT account_holder_id, created_at, id FROM deposit_accounts \
             WHERE account_holder_id = ANY($1) \
             ORDER BY account_holder_id, created_at DESC, id DESC\""
        ));
        assert!(tokens.contains(
            "WHERE account_holder_id = ANY($1) \
             ORDER BY account_holder_id, created_at ASC, id ASC\""
        ));
        assert!(tokens.contains("fetch_n (op , usize :: MAX)"));
    }

    #[test]
    fn by_id_orders_by_key_then_id() {
        let tokens = tokens(deposit_accounts(quote::quote! {}));

        assert!(tokens.contains(
            "SELECT currency, id FROM deposit_accounts WHERE currency = ANY($1) \
             ORDER BY currency, id DESC"
        ));
    }

    #[test]
    fn scoped_repo_adds_the_scope_conjunct_at_param_two() {
        let tokens = tokens(parse_quote! {
            #[es_repo(
                entity = "DepositAccount",
                columns(
                    owner_id(ty = "OwnerId", scope),
                    currency(ty = "Currency", list_for(by(created_at), find_all_for)),
                )
            )]
            struct DepositAccounts {
                pool: sqlx::PgPool,
            }
        });

        assert!(tokens.contains(
            "WHERE currency = ANY($1) AND owner_id = $2 \
             ORDER BY currency, created_at DESC, id DESC"
        ));
        assert!(tokens.contains("scope : impl Into < DepositAccountScope >"));
        assert!(tokens.contains("self . repo . find_all_for_currencies (self . scope ,"));
        assert!(
            tokens.contains("self . repo . find_all_for_currencies_in_op (op , self . scope ,")
        );
    }

    #[test]
    fn soft_delete_hides_deleted_unless_included() {
        let tokens = tokens(deposit_accounts(quote::quote! { delete = "soft", }));

        assert!(tokens.contains(
            "WHERE currency = ANY($1) AND deleted = FALSE \
             ORDER BY currency, id ASC"
        ));
        assert!(tokens.contains("pub async fn find_all_for_currencies_include_deleted <"));
        assert!(tokens.contains("pub async fn find_all_for_currencies_include_deleted_in_op <"));
        assert!(tokens.contains("WHERE currency = ANY($1) ORDER BY currency, id ASC"));
    }

    #[test]
    fn nested_soft_delete_repo_includes_deleted_children_only_on_include_deleted() {
        let tokens = tokens(parse_quote! {
            #[es_repo(
                entity = "Order",
                delete = "soft",
                columns(status(ty = "String", list_for(by(id), find_all_for)))
            )]
            struct Orders {
                pool: sqlx::PgPool,
                #[es_repo(nested)]
                items: OrderItems,
            }
        });

        let (visible, include_deleted) = tokens
            .split_once("pub async fn find_all_for_statuses_include_deleted <")
            .expect("include_deleted variant");
        assert!(visible.contains("fetch_n (op , usize :: MAX)"));
        assert!(!visible.contains("fetch_n_include_deleted"));
        assert!(include_deleted.contains("fetch_n_include_deleted (op , usize :: MAX)"));
    }

    #[test]
    fn in_op_only_suppresses_the_standalone_fns() {
        let tokens = tokens(deposit_accounts(quote::quote! { in_op_only, }));

        assert!(tokens.contains("pub async fn find_all_for_currencies_in_op <"));
        assert!(!tokens.contains("pub async fn find_all_for_currencies <"));
        assert!(!tokens.contains("pub async fn find_all_for_account_holder_ids <"));
    }

    #[test]
    fn list_for_without_the_flag_emits_no_batch_lookup() {
        let tokens = tokens(parse_quote! {
            #[es_repo(
                entity = "Doc",
                columns(
                    user_id(ty = "UserId", list_for(by(created_at))),
                    status(ty = "String", list_for)
                )
            )]
            struct Docs {
                pool: sqlx::PgPool,
            }
        });

        assert!(tokens.contains("list_for_user_id_by_created_at"));
        assert!(!tokens.contains("find_all_for_"));
    }

    #[test]
    fn key_defaults_to_the_entity_field_named_after_the_column() {
        let tokens = tokens(parse_quote! {
            #[es_repo(
                entity = "Doc",
                columns(
                    user_id(
                        ty = "UserId",
                        list_for(by(created_at), find_all_for),
                        create(accessor = "owner_id"),
                        update(accessor = "state")
                    )
                )
            )]
            struct Docs {
                pool: sqlx::PgPool,
            }
        });

        assert!(tokens.contains("let __key = __entity . user_id . clone ()"));
        assert!(!tokens.contains("__entity . owner_id"));
        assert!(!tokens.contains("__entity . state"));
    }

    #[test]
    fn accessor_expression_is_evaluated_on_the_entity() {
        let tokens = tokens(parse_quote! {
            #[es_repo(
                entity = "Entry",
                columns(
                    account_id(
                        ty = "AccountId",
                        list_for(by(created_at), find_all_for(accessor = "values().account_id")),
                        update(persist = false)
                    )
                )
            )]
            struct Entries {
                pool: sqlx::PgPool,
            }
        });

        assert!(tokens.contains("let __key = __entity . values () . account_id . clone ()"));
        assert!(tokens.contains("pub async fn find_all_for_account_ids <"));
    }

    #[test]
    fn flag_on_an_optional_column_is_an_error_naming_the_column() {
        let err = derive(parse_quote! {
            #[es_repo(
                entity = "DepositAccount",
                columns(
                    nickname(ty = "Option<String>", list_for(by(created_at), find_all_for))
                )
            )]
            struct DepositAccounts {
                pool: sqlx::PgPool,
            }
        })
        .expect_err("optional column cannot opt in")
        .to_string();

        assert!(err.contains("column 'nickname'"), "unexpected error: {err}");
        assert!(err.contains("find_all_for"), "unexpected error: {err}");
    }

    #[test]
    fn flag_on_a_nullable_virtual_or_by_less_column_is_an_error() {
        for (column, name) in [
            (
                quote::quote! { tier(ty = "Tier", nullable, list_for(by(created_at), find_all_for)) },
                "tier",
            ),
            (
                quote::quote! { flagged(ty = "bool", virtual = "TRUE", list_for(find_all_for)) },
                "flagged",
            ),
            (
                quote::quote! { currency(ty = "Currency", list_for(find_all_for)) },
                "currency",
            ),
        ] {
            let err = derive(parse_quote! {
                #[es_repo(entity = "DepositAccount", columns(#column))]
                struct DepositAccounts {
                    pool: sqlx::PgPool,
                }
            })
            .expect_err("column cannot opt in")
            .to_string();

            assert!(
                err.contains(&format!("column '{name}'")),
                "unexpected error: {err}"
            );
        }
    }

    #[test]
    fn repo_without_list_for_columns_emits_no_batch_lookup() {
        let tokens = tokens(parse_quote! {
            #[es_repo(entity = "User", columns(name(ty = "String", list_by)))]
            struct Users {
                pool: sqlx::PgPool,
            }
        });

        assert!(!tokens.contains("find_all_for_"));
    }
}
