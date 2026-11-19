use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::{
    events_write::{EventSource, EventsInsert, ForgettablePayloads},
    options::*,
};

/// Batch soft delete: `delete_in_op` applied to every entity of a `Vec`, in a
/// fixed number of statements regardless of the batch size. It is what the
/// parent repo's nested update uses to delete children removed with
/// `Nested::delete_persisted`.
pub struct DeleteAllFn<'a> {
    in_op_only: bool,
    id: &'a syn::Ident,
    event: &'a syn::Ident,
    entity: &'a syn::Ident,
    table_name: &'a str,
    events_table_name: &'a str,
    event_ctx: bool,
    columns: &'a Columns,
    delete_option: &'a DeleteOption,
    nested_delete_all_fn_names: Vec<syn::Ident>,
    post_persist_hook: bool,
    forgettable_table_name: Option<&'a str>,
    /// `RepositoryOptions::update_can_reject`: `false` narrows the statement's
    /// classification with `narrow_rejected()`.
    can_reject: bool,
    /// The generated signatures' error type, see
    /// `RepositoryOptions::update_error_ty`.
    write_error_ty: TokenStream,
    #[cfg(feature = "instrument")]
    repo_name_snake: String,
}

impl<'a> DeleteAllFn<'a> {
    pub fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            in_op_only: opts.in_op_only(),
            id: opts.id(),
            event: opts.event(),
            entity: opts.entity(),
            columns: &opts.columns,
            table_name: opts.table_name(),
            events_table_name: opts.events_table_name(),
            event_ctx: opts.event_context_enabled(),
            delete_option: &opts.delete,
            nested_delete_all_fn_names: opts
                .all_nested()
                .map(|f| f.delete_all_nested_fn_name())
                .collect(),
            post_persist_hook: opts.post_persist_hook.is_some(),
            forgettable_table_name: opts.forgettable_table_name(),
            can_reject: opts.update_can_reject(),
            write_error_ty: opts.update_error_ty(),
            #[cfg(feature = "instrument")]
            repo_name_snake: opts.repo_name_snake_case(),
        }
    }
}

impl ToTokens for DeleteAllFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        if !self.delete_option.is_soft() {
            return;
        }

        let entity = self.entity;
        let table_name = self.table_name;
        let id_type = self.id;
        let event_type = self.event;
        let write_error_ty = &self.write_error_ty;

        // The cascade runs once per nested field for the whole batch.
        let nested_deletes = self.nested_delete_all_fn_names.iter().map(|f| {
            quote! {
                Self::#f(op, &delete_ids).await?;
            }
        });

        // The payload purge runs *before* the combined statement, as in
        // `delete_in_op`: payloads staged but not yet persisted at delete time
        // survive the delete.
        let forget_payloads = self.forgettable_table_name.map(|forgettable_tbl| {
            let forget_query = format!("DELETE FROM {} WHERE entity_id = ANY($1)", forgettable_tbl);
            quote! {
                sqlx::query(#forget_query)
                    .bind(&delete_ids)
                    .execute(op.as_executor())
                    .await?;
            }
        });

        let payloads = self
            .forgettable_table_name
            .map(|table| ForgettablePayloads {
                table,
                id_type,
                event_type,
            });
        let forgettable_vars = payloads
            .as_ref()
            .map(|p| p.batch_declarations())
            .unwrap_or_default();
        let forgettable_extract = payloads
            .as_ref()
            .map(|p| p.gather_batch(quote! { entity.events() }, quote! { &entity.id }))
            .unwrap_or_default();
        let forgettable_insert = payloads
            .as_ref()
            .map(|p| p.insert_batch(self.events_table_name))
            .unwrap_or_default();

        // Every entity goes into the index arrays (a delete flags the row even
        // when no event is staged); only entities with staged events add event
        // rows, `gather_batch` pushes nothing for the others.
        let events_insert = EventsInsert::new(self.events_table_name, self.event_ctx);
        let (vec_declarations, per_entity_pushes, bind_tokens) = self
            .columns
            .delete_all_arg_parts(syn::parse_quote! { entity });
        let set_parts = self.columns.sql_bulk_update_set_for_delete();
        let column_names = self.columns.delete_all_column_names();
        let n_columns = column_names.len();
        let placeholders = (1..=n_columns)
            .map(|i| format!("${i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let column_list = column_names.join(", ");

        let now_p = n_columns + 1;
        let source = EventSource::BatchCte { cte: "updated" };
        let query = format!(
            "WITH updated AS (UPDATE {table_name} SET {set_parts}deleted = TRUE \
                 FROM UNNEST({placeholders}) \
                 AS unnested({column_list}) \
                 WHERE {table_name}.id = unnested.id RETURNING {table_name}.id) {}",
            events_insert.sql(&source, now_p, now_p + 1),
        );
        let event_binds = events_insert
            .arg_exprs(&source)
            .into_iter()
            .map(|expr| quote! { .bind(#expr) });
        let batch_declarations = events_insert.batch_declarations(id_type);
        let gather = events_insert.gather_batch(quote! { entity.events() }, quote! { &entity.id });

        let (narrow_import, narrow_call) = if self.can_reject {
            (quote! {}, quote! {})
        } else {
            (
                quote! { use es_entity::errlanes::ResultExt as _; },
                quote! { .narrow_rejected() },
            )
        };

        #[cfg(feature = "instrument")]
        let (instrument_attr, error_recording) = {
            let entity_name = entity.to_string();
            let repo_name = &self.repo_name_snake;
            let span_name = format!("{}.delete_all", repo_name);
            (
                quote! {
                    #[tracing::instrument(name = #span_name, skip_all, fields(entity = #entity_name, count = entities.len(), error = tracing::field::Empty, exception.message = tracing::field::Empty, exception.type = tracing::field::Empty))]
                },
                quote! {
                    if let Err(ref e) = __result {
                        tracing::Span::current().record("error", true);
                        tracing::Span::current().record("exception.message", tracing::field::display(e));
                        tracing::Span::current().record("exception.type", std::any::type_name_of_val(e));
                    }
                },
            )
        };
        #[cfg(not(feature = "instrument"))]
        let (instrument_attr, error_recording) = (quote! {}, quote! {});

        let post_persist_check = if self.post_persist_hook {
            quote! {
                for entity in entities.iter() {
                    if let Some(&n_events) = n_persisted.get(&entity.id) {
                        if n_events > 0 {
                            self.execute_post_persist_hook(op, entity, entity.events().last_persisted(n_events)).await?;
                        }
                    }
                }
            }
        } else {
            quote! {}
        };

        let standalone = (!self.in_op_only).then(|| {
            quote! {
                pub async fn delete_all(
                    &self,
                    entities: Vec<#entity>
                ) -> Result<(), #write_error_ty> {
                    let mut op = self.begin_op().await?;
                    self.delete_all_in_op(&mut op, entities).await?;
                    op.commit().await?;
                    Ok(())
                }
            }
        });

        tokens.append_all(quote! {
            #standalone

            #instrument_attr
            pub async fn delete_all_in_op<OP>(
                &self,
                op: &mut OP,
                mut entities: Vec<#entity>
            ) -> Result<(), #write_error_ty>
            where
                OP: es_entity::AtomicOperation + ?Sized
            {
                let __result: Result<(), #write_error_ty> = async {
                    use es_entity::prelude::sqlx::Row;
                    #narrow_import

                    if entities.is_empty() {
                        return Ok(());
                    }

                    let delete_ids: Vec<#id_type> = entities.iter().map(|entity| entity.id.clone()).collect();

                    #(#nested_deletes)*

                    #forget_payloads

                    #vec_declarations
                    #batch_declarations
                    let mut n_persisted: std::collections::HashMap<#id_type, usize> = std::collections::HashMap::new();
                    #forgettable_vars

                    for entity in entities.iter() {
                        #per_entity_pushes
                        #gather
                        #forgettable_extract
                        n_persisted.insert(entity.id.clone(), n_new);
                    }

                    let expected_events = all_ids.len();
                    let rows = sqlx::query(#query)
                        #(#bind_tokens)*
                        #(#event_binds)*
                        .fetch_all(op.as_executor())
                        .await
                        .map_err(|e| Self::classify_update_write(e, format!("{} batch conflict", #table_name)))
                        #narrow_call?;

                    #forgettable_insert

                    // Only entities with staged events produce rows. Every event
                    // row joins an index row this same statement updated, so a
                    // short count means a row went missing — a lost race with a
                    // concurrent delete, not an internal error.
                    if expected_events > 0 {
                        if rows.len() != expected_events {
                            return Err(errlanes::Transient::new(errlanes::TransientKind::OptimisticConflict)
                                .with_context(format!(
                                    "{} batch wrote {} of {} events",
                                    #table_name,
                                    rows.len(),
                                    expected_events
                                ))
                                .into());
                        }

                        let recorded_at = rows
                            .first()
                            .ok_or(sqlx::Error::RowNotFound)
                            .and_then(|row| row.try_get("recorded_at"))?;
                        for entity in entities.iter_mut() {
                            let events = Self::extract_events(entity);
                            if events.any_new() {
                                events.mark_new_events_persisted_at(recorded_at);
                            }
                        }

                        #post_persist_check
                    }

                    Ok(())
                }.await;

                #error_recording
                __result
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use darling::FromDeriveInput;
    use syn::parse_quote;

    use super::*;

    fn output(input: syn::DeriveInput) -> String {
        let opts = RepositoryOptions::from_derive_input(&input).unwrap();
        let mut tokens = TokenStream::new();
        DeleteAllFn::from(&opts).to_tokens(&mut tokens);
        tokens.to_string()
    }

    #[test]
    fn no_update_columns_unnests_only_the_id() {
        let out = output(parse_quote! {
            #[es_repo(entity = "Order", delete = "soft")]
            struct Orders { pool: sqlx::PgPool }
        });
        assert!(out.contains(
            "WITH updated AS (UPDATE orders SET deleted = TRUE FROM UNNEST($1) AS unnested(id) WHERE orders.id = unnested.id RETURNING orders.id) INSERT INTO order_events"
        ));
        assert!(out.contains("pub async fn delete_all ("));
        assert!(out.contains("pub async fn delete_all_in_op"));
        assert!(out.contains("if entities . is_empty ()"));
        // Events join the CTE with the now-placeholder right after the columns.
        assert!(out.contains("COALESCE($2, NOW())"));
        assert!(out.contains("UNNEST($3, $4::INT[], $5::TEXT[], $6::JSONB[])"));
        assert!(!out.contains("sqlx :: query !"));
    }

    #[test]
    fn update_column_is_unnested_and_set() {
        let out = output(parse_quote! {
            #[es_repo(
                entity = "OrderItem",
                delete = "soft",
                columns(
                    order_id(ty = "OrderId", update(persist = false), parent),
                    status(ty = "String")
                )
            )]
            struct OrderItems { pool: sqlx::PgPool }
        });
        assert!(out.contains(
            "UPDATE order_items SET status = unnested.status, deleted = TRUE FROM UNNEST($1, $2) AS unnested(id, status)"
        ));
        assert!(out.contains("COALESCE($3, NOW())"));
    }

    #[test]
    fn forgettable_column_is_nulled_and_not_unnested() {
        let out = output(parse_quote! {
            #[es_repo(
                entity = "AccountHolder",
                forgettable,
                delete = "soft",
                columns(email(ty = "Forgettable<String>"))
            )]
            struct AccountHolders { pool: sqlx::PgPool }
        });
        assert!(out.contains(
            "UPDATE account_holders SET email = NULL, deleted = TRUE FROM UNNEST($1) AS unnested(id)"
        ));
        assert!(out.contains(
            "DELETE FROM account_holders_forgettable_payloads WHERE entity_id = ANY($1)"
        ));
        assert!(out.contains("payload_sequences"));
    }

    #[test]
    fn event_context_adds_context_array() {
        let out = output(parse_quote! {
            #[es_repo(entity = "Order", delete = "soft", persist_event_context)]
            struct Orders { pool: sqlx::PgPool }
        });
        assert!(out.contains("all_contexts"));
        assert!(out.contains("context"));
    }

    #[test]
    fn nested_fields_cascade_once_for_the_batch() {
        let out = output(parse_quote! {
            #[es_repo(entity = "Order", delete = "soft")]
            struct Orders {
                pool: sqlx::PgPool,
                #[es_repo(nested)]
                items: OrderItems,
            }
        });
        assert!(out.contains("Self :: delete_all_nested_items_in_op (op , & delete_ids)"));
    }

    #[test]
    fn post_persist_hook_runs_for_entities_with_events() {
        let out = output(parse_quote! {
            #[es_repo(entity = "Order", delete = "soft", post_persist_hook = "on_persist")]
            struct Orders { pool: sqlx::PgPool }
        });
        assert!(out.contains("execute_post_persist_hook"));
    }

    #[test]
    fn non_soft_repo_gets_no_delete_all() {
        let out = output(parse_quote! {
            #[es_repo(entity = "Order")]
            struct Orders { pool: sqlx::PgPool }
        });
        assert!(out.is_empty());
    }

    #[test]
    fn in_op_only_has_no_standalone_delete_all() {
        let out = output(parse_quote! {
            #[es_repo(entity = "Order", delete = "soft", in_op_only)]
            struct Orders { pool: sqlx::PgPool }
        });
        assert!(!out.contains("pub async fn delete_all ("));
        assert!(out.contains("pub async fn delete_all_in_op"));
    }
}
