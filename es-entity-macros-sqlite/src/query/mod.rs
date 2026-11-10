mod input;

use convert_case::{Case, Casing};
use darling::ToTokens;
use proc_macro2::{Span, TokenStream};
use quote::{TokenStreamExt, quote};

pub use input::QueryInput;

pub fn expand(input: QueryInput) -> darling::Result<proc_macro2::TokenStream> {
    let query = EsQuery::from(input);
    Ok(quote!(#query))
}

pub struct EsQuery {
    input: QueryInput,
}

impl From<QueryInput> for EsQuery {
    fn from(input: QueryInput) -> Self {
        Self { input }
    }
}

impl ToTokens for EsQuery {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let singular = pluralizer::pluralize(
            &self
                .input
                .table_name()
                .expect("Could not identify table name"),
            1,
            false,
        );
        let entity = if let Some(entity_ty) = &self.input.entity {
            entity_ty.clone()
        } else {
            let singular_without_prefix = pluralizer::pluralize(
                &self
                    .input
                    .table_name_without_prefix()
                    .expect("Could not identify table name"),
                1,
                false,
            );
            syn::Ident::new(
                &singular_without_prefix.to_case(Case::UpperCamel),
                Span::call_site(),
            )
        };

        let entity_snake = entity.to_string().to_case(Case::Snake);
        let repo_types_mod =
            syn::Ident::new(&format!("{entity_snake}_repo_types"), Span::call_site());
        let order_by = self.input.order_by();

        let events_table = syn::Ident::new(&format!("{singular}_events"), Span::call_site());
        let args = &self.input.arg_exprs;
        let n_args = args.len();
        let context_arg = format!("${}", n_args + 1);
        let user_sql = &self.input.sql;
        let raw_order_cols = self.input.order_by_columns_raw();

        // SQLite: runtime query decoded by name through
        // `decode_tagged_row`, so the columns carry plain aliases rather than
        // `query_as!` type overrides.
        let (payload_column, forgettable_join) =
            if let Some(ref forgettable_tbl) = self.input.forgettable_tbl {
                (
                    "p.payload AS forgettable_payload".to_string(),
                    format!(
                        " LEFT JOIN {} p ON e.id = p.entity_id AND e.sequence = p.sequence",
                        forgettable_tbl
                    ),
                )
            } else {
                ("NULL AS forgettable_payload".to_string(), String::new())
            };

        let default_fingerprint: syn::Expr = syn::parse_quote! {
            <<<Self as es_entity::EsRepo>::Entity as es_entity::EsEntity>::Snapshot as es_entity::EsSnapshot>::FINGERPRINT
        };
        let fingerprint_expr = self
            .input
            .snapshot_fingerprint
            .clone()
            .unwrap_or(default_fingerprint);

        let (query, decode_fn) = if let Some(ref snapshot_tbl) = self.input.snapshot_tbl {
            let fingerprint_arg = format!("${}", n_args + 2);
            let (snapshot_payload_column, snapshot_payload_join) = match self.input.forgettable_tbl
            {
                Some(ref forgettable_tbl) => (
                    "sp.payload AS snapshot_forgettable_payload".to_string(),
                    format!(
                        " LEFT JOIN {forgettable_tbl} sp ON sp.entity_id = i.id AND sp.sequence = 0"
                    ),
                ),
                None => (
                    "NULL AS snapshot_forgettable_payload".to_string(),
                    String::new(),
                ),
            };
            let query = format!(
                "WITH entities AS ({}) SELECT i.id AS entity_id, COALESCE(e.sequence, s.sequence) AS sequence, e.event AS event, CASE WHEN {} THEN e.context ELSE NULL END AS context, e.recorded_at AS recorded_at, {}, CASE WHEN e.sequence IS NULL OR e.sequence = s.sequence + 1 THEN s.snapshot END AS snapshot, s.sequence AS snapshot_sequence, s.recorded_at AS snapshot_recorded_at, s.first_recorded_at AS snapshot_first_recorded_at, {} FROM entities i LEFT JOIN {} s ON s.id = i.id AND s.fingerprint = {} LEFT JOIN {} e ON e.id = i.id AND e.sequence > COALESCE(s.sequence, 0){}{} ORDER BY {} COALESCE(e.sequence, s.sequence)",
                self.input.sql,
                context_arg,
                payload_column,
                snapshot_payload_column,
                snapshot_tbl,
                fingerprint_arg,
                events_table,
                forgettable_join,
                snapshot_payload_join,
                order_by,
            );
            (
                query,
                quote! { es_entity::decode_tagged_snapshot_row::<Repo__Id> },
            )
        } else {
            let query = format!(
                "WITH entities AS ({}) SELECT i.id AS entity_id, e.sequence AS sequence, e.event AS event, CASE WHEN {} THEN e.context ELSE NULL END AS context, e.recorded_at AS recorded_at, {} FROM entities i JOIN {} e ON i.id = e.id{} ORDER BY {} e.sequence",
                self.input.sql,
                context_arg,
                payload_column,
                events_table,
                forgettable_join,
                order_by
            );
            (query, quote! { es_entity::decode_tagged_row::<Repo__Id> })
        };

        let mut bind_exprs: Vec<TokenStream> = args.iter().map(|a| quote! { #a }).collect();
        bind_exprs.push(quote! {
            <<<Self as es_entity::EsRepo>::Entity as EsEntity>::Event>::event_context()
        });
        if self.input.snapshot_tbl.is_some() {
            bind_exprs.push(quote! { #fingerprint_expr });
        }
        let user_sql = crate::sqlite_sql::translate(user_sql);
        // A slice parameter is expanded at runtime (SQLite cannot bind
        // arrays): its elements bind first, the other placeholders shift.
        let (list_prelude, runtime_query, n_user_args, user_sql_override) = if self
            .input
            .expand_first_arg
            && !bind_exprs.is_empty()
        {
            let first = bind_exprs.remove(0);
            let query = crate::sqlite_sql::translate(&query);
            let args = crate::runtime_query::args_with_list(&quote! { __list }, &bind_exprs);
            (
                quote! { let __list = #first; let __n = __list.len(); },
                quote! {
                    sqlx::__query_with_result::<es_entity::db::Db, _>(
                        sqlx::AssertSqlSafe(es_entity::sqlite_write::expand_list_param(#query, __n)),
                        #args,
                    )
                },
                quote! { #n_args + __n - 1 },
                quote! {
                    .with_user_sql(es_entity::sqlite_write::expand_list_param(#user_sql, __n))
                },
            )
        } else {
            (
                quote! {},
                crate::runtime_query::query(&query, &bind_exprs),
                quote! { #n_args },
                quote! {},
            )
        };

        let forgettable_check = if self.input.forgettable_tbl.is_none() {
            quote! {
                const _: () = assert!(
                    !Repo__Event::HAS_FORGETTABLE_FIELDS,
                    "es_query! requires `forgettable_tbl` parameter when the event type has Forgettable<T> fields"
                );
            }
        } else {
            quote! {}
        };

        let tbl_prefix_check = if self.input.tbl_prefix.is_none() && self.input.entity.is_none() {
            quote! {
                const _: () = assert!(
                    !REPO__HAS_TBL_PREFIX,
                    "es_query! requires `tbl_prefix` parameter when the repo uses tbl_prefix"
                );
            }
        } else {
            quote! {}
        };

        let snapshot_tbl_check = if self.input.snapshot_tbl.is_none() {
            quote! {
                const _: () = assert!(
                    !REPO__HAS_SNAPSHOT,
                    "es_query! on a `snapshot` repo requires the `snapshot_tbl` parameter"
                );
            }
        } else {
            quote! {
                const _: () = assert!(
                    REPO__HAS_SNAPSHOT,
                    "es_query! `snapshot_tbl` parameter given but this repo does not enable `snapshot`"
                );
            }
        };

        tokens.append_all(quote! {
            {
                use #repo_types_mod::*;

                #forgettable_check
                #tbl_prefix_check
                #snapshot_tbl_check

                #list_prelude
                es_entity::EsQuery::<Self, <Self as es_entity::EsRepo>::EsQueryFlavor, _, _, es_entity::HydrationRow<Repo__Id>>::new(
                    #runtime_query.try_map(|__row: es_entity::db::Row| #decode_fn(&__row)),
                    es_entity::TreeQuerySource {
                        user_sql: #user_sql,
                        order_by_cols: &[#(#raw_order_cols),*],
                        n_user_args: #n_user_args,
                        decode: #decode_fn,
                        snapshot_fingerprint: #fingerprint_expr,
                    },
                )
                #user_sql_override
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    fn expand(input: QueryInput) -> String {
        let query = EsQuery::from(input);
        let mut tokens = TokenStream::new();
        query.to_tokens(&mut tokens);
        tokens.to_string()
    }

    /// A runtime query (no compile-time `query_as!`), SQLite placeholders,
    /// plain column aliases, and every argument bound in placeholder order.
    #[test]
    fn query() {
        let out = expand(parse_quote!(
            sql = "SELECT * FROM users WHERE id = $1",
            args = [id as UserId]
        ));
        assert!(!out.contains("query_as !"), "{out}");
        assert!(
            out.contains("sqlx :: __query_with_result :: < es_entity :: db :: Db , _ >"),
            "{out}"
        );
        assert!(out.contains(
            "\"WITH entities AS (SELECT * FROM users WHERE id = ?1) SELECT i.id AS entity_id, e.sequence AS sequence, e.event AS event, CASE WHEN ?2 THEN e.context ELSE NULL END AS context, e.recorded_at AS recorded_at, NULL AS forgettable_payload FROM entities i JOIN user_events e ON i.id = e.id ORDER BY i.id, e.sequence\""
        ), "{out}");
        assert!(out.contains("& (id as UserId)"), "{out}");
        assert!(out.contains("event_context ()"), "{out}");
        assert!(out.contains("use user_repo_types :: *"), "{out}");
        assert!(
            out.contains("user_sql : \"SELECT * FROM users WHERE id = ?1\""),
            "{out}"
        );
        assert!(out.contains("n_user_args : 1usize"), "{out}");
        assert!(out.contains("decode_tagged_row :: < Repo__Id >"), "{out}");
    }

    #[test]
    fn query_with_entity_ty() {
        let out = expand(parse_quote!(
            entity = MyCustomEntity,
            sql = "SELECT * FROM my_custom_table WHERE id = $1",
            args = [id as MyCustomEntityId]
        ));
        assert!(
            out.contains("use my_custom_entity_repo_types :: *"),
            "{out}"
        );
        assert!(out.contains("JOIN my_custom_table_events e"), "{out}");
        assert!(!out.contains("REPO__HAS_TBL_PREFIX"), "{out}");
    }

    #[test]
    fn query_with_order() {
        let out = expand(parse_quote!(
            sql = "SELECT name, id FROM entities WHERE ((name, id) > ($3, $2)) OR $2 IS NULL ORDER BY name, id LIMIT $1",
            args = [
                (first + 1) as i64,
                id as Option<MyCustomEntityId>,
                name as Option<String>
            ]
        ));
        assert!(out.contains(
            "WITH entities AS (SELECT name, id FROM entities WHERE ((name, id) > (?3, ?2)) OR ?2 IS NULL ORDER BY name, id LIMIT ?1)"
        ), "{out}");
        assert!(out.contains("CASE WHEN ?4 THEN e.context"), "{out}");
        assert!(
            out.contains("ORDER BY i.name, i.id, i.id, e.sequence"),
            "{out}"
        );
        assert!(
            out.contains("order_by_cols : & [\"name\" , \"id\"]"),
            "{out}"
        );
    }

    /// `expand_first_arg`: the slice is expanded into `IN (…)` at runtime and
    /// bound element by element ahead of the other arguments.
    #[test]
    fn query_with_key_any_and_directional_order_groups_rows_per_key() {
        let out = expand(parse_quote!(
            expand_first_arg = true,
            sql = "SELECT account_holder_id, created_at, id FROM entities WHERE account_holder_id = ANY($1) ORDER BY account_holder_id, created_at DESC, id DESC",
            args = [account_holder_ids as &[MyCustomEntityId]]
        ));
        assert!(out.contains(
            "WITH entities AS (SELECT account_holder_id, created_at, id FROM entities WHERE account_holder_id = ANY(?1) ORDER BY account_holder_id, created_at DESC, id DESC) SELECT"
        ), "{out}");
        assert!(
            out.contains("let __list = account_holder_ids as & [MyCustomEntityId]"),
            "{out}"
        );
        assert!(
            out.contains("es_entity :: sqlite_write :: expand_list_param"),
            "{out}"
        );
        assert!(out.contains("n_user_args : 1usize + __n - 1"), "{out}");
        assert!(
            out.contains(
                "ORDER BY i.account_holder_id, i.created_at desc, i.id desc, i.id, e.sequence"
            ),
            "{out}"
        );
        assert!(
            out.contains(
                "order_by_cols : & [\"account_holder_id\" , \"created_at desc\" , \"id desc\"]"
            ),
            "{out}"
        );
    }
}
