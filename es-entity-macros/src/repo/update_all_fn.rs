use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::{
    events_write::{EventSource, EventsInsert, ForgettablePayloads},
    options::*,
};

pub struct UpdateAllFn<'a> {
    entity: &'a syn::Ident,
    id: &'a syn::Ident,
    event: &'a syn::Ident,
    table_name: &'a str,
    events_table_name: &'a str,
    event_ctx: bool,
    forgettable_table_name: Option<&'a str>,
    columns: &'a Columns,
    modify_error: syn::Ident,
    nested_fn_names: Vec<syn::Ident>,
    post_persist_error: Option<&'a syn::Type>,
    #[cfg(feature = "instrument")]
    repo_name_snake: String,
}

impl<'a> From<&'a RepositoryOptions> for UpdateAllFn<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            entity: opts.entity(),
            id: opts.id(),
            event: opts.event(),
            modify_error: opts.modify_error(),
            columns: &opts.columns,
            table_name: opts.table_name(),
            events_table_name: opts.events_table_name(),
            event_ctx: opts.event_context_enabled(),
            forgettable_table_name: opts.forgettable_table_name(),
            nested_fn_names: opts
                .all_nested()
                .map(|f| f.update_nested_fn_name())
                .collect(),
            post_persist_error: opts.post_persist_hook.as_ref().map(|h| &h.error),
            #[cfg(feature = "instrument")]
            repo_name_snake: opts.repo_name_snake_case(),
        }
    }
}

impl ToTokens for UpdateAllFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let entity = self.entity;
        let modify_error = &self.modify_error;

        let nested = self.nested_fn_names.iter().map(|f| {
            quote! {
                self.#f(op, entity).await?;
            }
        });

        let nested_phase = if self.nested_fn_names.is_empty() {
            None
        } else {
            let nested = nested.collect::<Vec<_>>();
            Some(quote! {
                for entity in entities.iter_mut() {
                    #(#nested)*
                }
            })
        };

        let id_type = self.id;

        let events_insert = EventsInsert::new(self.events_table_name, self.event_ctx);

        let payloads = self
            .forgettable_table_name
            .map(|table| ForgettablePayloads {
                table,
                id_type,
                event_type: self.event,
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
            .map(|p| p.insert_batch(modify_error))
            .unwrap_or_default();

        // Every entity in the batch is only borrowed here, so the index columns
        // and the event arrays can be gathered in the same pass and written by
        // a single statement; the events insert joins the `updated` CTE, which
        // orders the index write first and detects rows that vanished.
        let (vec_declarations, per_entity_pushes, persist_tokens) = if self.columns.updates_needed()
        {
            let (vecs, pushes, bind_tokens) = self
                .columns
                .update_all_arg_parts(syn::parse_quote! { entity });
            let set_clause = self.columns.sql_bulk_update_set();
            let column_names = self.columns.update_all_column_names();
            let n_columns = column_names.len();
            let placeholders = (1..=n_columns)
                .map(|i| format!("${i}"))
                .collect::<Vec<_>>()
                .join(", ");
            let column_list = column_names.join(", ");
            let table_name = self.table_name;

            let now_p = n_columns + 1;
            let source = EventSource::BatchCte { cte: "updated" };
            let query = format!(
                "WITH updated AS (UPDATE {table_name} SET {set_clause} \
                     FROM UNNEST({placeholders}) \
                     AS unnested({column_list}) \
                     WHERE {table_name}.id = unnested.id RETURNING {table_name}.id) {}",
                events_insert.sql(&source, now_p, now_p + 1),
            );

            let event_binds = events_insert
                .arg_exprs(&source)
                .into_iter()
                .map(|expr| quote! { .bind(#expr) });

            (
                Some(vecs),
                Some(pushes),
                quote! {
                    let expected_events = all_ids.len();
                    let rows = sqlx::query(#query)
                        #(#bind_tokens)*
                        #(#event_binds)*
                        .fetch_all(op.as_executor())
                        .await
                        .map_err(Self::classify_write_error)?;

                    #forgettable_insert

                    // Every event row joins an index row this same statement
                    // updated, so a short count means a row went missing.
                    if rows.len() != expected_events {
                        return Err(#modify_error::ConcurrentModification);
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
                },
            )
        } else {
            (
                None,
                None,
                quote! {
                    let mut all_event_refs: Vec<_> = entities.iter_mut()
                        .filter_map(|entity| {
                            let events = Self::extract_events(entity);
                            if events.any_new() { Some(events) } else { None }
                        })
                        .collect();
                    let n_persisted = Self::extract_concurrent_modification(
                        self.persist_events_batch(op, &mut all_event_refs).await,
                        #modify_error::ConcurrentModification,
                    )?;
                    drop(all_event_refs);
                },
            )
        };

        // Gathering of the event arrays only happens for the combined path;
        // the no-index-column path defers entirely to `persist_events_batch`.
        let (event_collection_vars, event_collection_pushes) = if self.columns.updates_needed() {
            let batch_declarations = events_insert.batch_declarations(id_type);
            let gather =
                events_insert.gather_batch(quote! { entity.events() }, quote! { &entity.id });
            (
                quote! {
                    #batch_declarations
                    let mut n_persisted: std::collections::HashMap<#id_type, usize> = std::collections::HashMap::new();
                    #forgettable_vars
                },
                quote! {
                    #gather
                    #forgettable_extract
                    n_persisted.insert(entity.id.clone(), n_new);
                },
            )
        } else {
            (quote! {}, quote! {})
        };

        #[cfg(feature = "instrument")]
        let (instrument_attr, error_recording) = {
            let entity_name = entity.to_string();
            let repo_name = &self.repo_name_snake;
            let span_name = format!("{}.update_all", repo_name);
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

        let post_persist_check = if self.post_persist_error.is_some() {
            quote! {
                self.execute_post_persist_hook(op, &entity, entity.events().last_persisted(n_events)).await.map_err(#modify_error::PostPersistHookError)?;
            }
        } else {
            quote! {}
        };

        tokens.append_all(quote! {
            pub async fn update_all(
                &self,
                entities: &mut [#entity]
            ) -> Result<usize, #modify_error> {
                let mut op = self.begin_op().await?;
                let res = self.update_all_in_op(&mut op, entities).await?;
                op.commit().await?;
                Ok(res)
            }

            #instrument_attr
            pub async fn update_all_in_op<OP>(
                &self,
                op: &mut OP,
                entities: &mut [#entity]
            ) -> Result<usize, #modify_error>
            where
                OP: es_entity::AtomicOperation
            {
                let __result: Result<usize, #modify_error> = async {
                    use es_entity::prelude::sqlx::Row;

                    if entities.is_empty() {
                        return Ok(0);
                    }

                    #nested_phase

                    #vec_declarations
                    #event_collection_vars

                    let mut has_new_events = false;
                    for entity in entities.iter() {
                        if !entity.events().any_new() {
                            continue;
                        }
                        has_new_events = true;

                        #per_entity_pushes
                        #event_collection_pushes
                    }

                    if !has_new_events {
                        return Ok(0);
                    }

                    #persist_tokens

                    let mut total_events = 0usize;
                    for entity in entities.iter_mut() {
                        if let Some(&n_events) = n_persisted.get(&entity.id) {
                            if n_events > 0 {
                                #post_persist_check
                                total_events += n_events;
                            }
                        }
                    }

                    Ok(total_events)
                }.await;

                #error_recording
                __result
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proc_macro2::Span;
    use syn::Ident;

    #[test]
    fn update_all_fn() {
        let id = syn::parse_str("EntityId").unwrap();
        let entity = Ident::new("Entity", Span::call_site());

        let columns = Columns::new(
            &id,
            [Column::new(
                Ident::new("name", Span::call_site()),
                syn::parse_str("String").unwrap(),
            )],
        );

        let event = Ident::new("EntityEvent", Span::call_site());
        let update_all_fn = UpdateAllFn {
            entity: &entity,
            id: &id,
            event: &event,
            table_name: "entities",
            events_table_name: "entity_events",
            event_ctx: false,
            forgettable_table_name: None,
            modify_error: syn::Ident::new("EntityModifyError", Span::call_site()),
            columns: &columns,
            nested_fn_names: Vec::new(),
            post_persist_error: None,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        update_all_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn update_all(
                &self,
                entities: &mut [Entity]
            ) -> Result<usize, EntityModifyError> {
                let mut op = self.begin_op().await?;
                let res = self.update_all_in_op(&mut op, entities).await?;
                op.commit().await?;
                Ok(res)
            }

            pub async fn update_all_in_op<OP>(
                &self,
                op: &mut OP,
                entities: &mut [Entity]
            ) -> Result<usize, EntityModifyError>
            where
                OP: es_entity::AtomicOperation
            {
                let __result: Result<usize, EntityModifyError> = async {
                    use es_entity::prelude::sqlx::Row;

                    if entities.is_empty() {
                        return Ok(0);
                    }

                    let mut id_collection = Vec::new();
                    let mut name_collection = Vec::new();

                    let mut all_ids: Vec<&EntityId> = Vec::new();
                    let mut all_sequences: Vec<i32> = Vec::new();
                    let mut all_types = Vec::new();
                    let mut all_serialized = Vec::new();
                    let mut n_persisted: std::collections::HashMap<EntityId, usize> = std::collections::HashMap::new();

                    let mut has_new_events = false;
                    for entity in entities.iter() {
                        if !entity.events().any_new() {
                            continue;
                        }
                        has_new_events = true;

                        let id = &entity.id;
                        let name = &entity.name;
                        id_collection.push(id);
                        name_collection.push(name);
                        let offset = entity.events().len_persisted() + 1;
                        let types = entity.events().new_event_types();
                        let serialized = entity.events().serialize_new_events();

                        let n_new = serialized.len();
                        all_types.extend(types);
                        all_serialized.extend(serialized);
                        all_ids.extend(std::iter::repeat(&entity.id).take(n_new));
                        all_sequences.extend((offset..).take(n_new).map(|i| i as i32));
                        n_persisted.insert(entity.id.clone(), n_new);
                    }

                    if !has_new_events {
                        return Ok(0);
                    }

                    let expected_events = all_ids.len();
                    let rows = sqlx::query("WITH updated AS (UPDATE entities SET name = unnested.name FROM UNNEST($1, $2) AS unnested(id, name) WHERE entities.id = unnested.id RETURNING entities.id) INSERT INTO entity_events (id, recorded_at, sequence, event_type, event) SELECT unnested.id, COALESCE($3, NOW()), unnested.sequence, unnested.event_type, unnested.event FROM UNNEST($4, $5::INT[], $6::TEXT[], $7::JSONB[]) AS unnested(id, sequence, event_type, event) JOIN updated ON updated.id = unnested.id RETURNING recorded_at")
                        .bind(id_collection)
                        .bind(name_collection)
                        .bind(op.maybe_now())
                        .bind(&all_ids)
                        .bind(&all_sequences)
                        .bind(&all_types)
                        .bind(&all_serialized)
                        .fetch_all(op.as_executor())
                        .await
                        .map_err(Self::classify_write_error)?;

                    if rows.len() != expected_events {
                        return Err(EntityModifyError::ConcurrentModification);
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

                    let mut total_events = 0usize;
                    for entity in entities.iter_mut() {
                        if let Some(&n_events) = n_persisted.get(&entity.id) {
                            if n_events > 0 {
                                total_events += n_events;
                            }
                        }
                    }

                    Ok(total_events)
                }.await;

                __result
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn update_all_fn_no_columns() {
        let id = syn::parse_str("EntityId").unwrap();
        let entity = Ident::new("Entity", Span::call_site());

        let mut columns = Columns::default();
        columns.set_id_column(&id);

        let event = Ident::new("EntityEvent", Span::call_site());
        let update_all_fn = UpdateAllFn {
            entity: &entity,
            id: &id,
            event: &event,
            table_name: "entities",
            events_table_name: "entity_events",
            event_ctx: false,
            forgettable_table_name: None,
            modify_error: syn::Ident::new("EntityModifyError", Span::call_site()),
            columns: &columns,
            nested_fn_names: Vec::new(),
            post_persist_error: None,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        update_all_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn update_all(
                &self,
                entities: &mut [Entity]
            ) -> Result<usize, EntityModifyError> {
                let mut op = self.begin_op().await?;
                let res = self.update_all_in_op(&mut op, entities).await?;
                op.commit().await?;
                Ok(res)
            }

            pub async fn update_all_in_op<OP>(
                &self,
                op: &mut OP,
                entities: &mut [Entity]
            ) -> Result<usize, EntityModifyError>
            where
                OP: es_entity::AtomicOperation
            {
                let __result: Result<usize, EntityModifyError> = async {
                    use es_entity::prelude::sqlx::Row;

                    if entities.is_empty() {
                        return Ok(0);
                    }

                    let mut has_new_events = false;
                    for entity in entities.iter() {
                        if !entity.events().any_new() {
                            continue;
                        }
                        has_new_events = true;
                    }

                    if !has_new_events {
                        return Ok(0);
                    }

                    let mut all_event_refs: Vec<_> = entities.iter_mut()
                        .filter_map(|entity| {
                            let events = Self::extract_events(entity);
                            if events.any_new() { Some(events) } else { None }
                        })
                        .collect();
                    let n_persisted = Self::extract_concurrent_modification(
                        self.persist_events_batch(op, &mut all_event_refs).await,
                        EntityModifyError::ConcurrentModification,
                    )?;
                    drop(all_event_refs);

                    let mut total_events = 0usize;
                    for entity in entities.iter_mut() {
                        if let Some(&n_events) = n_persisted.get(&entity.id) {
                            if n_events > 0 {
                                total_events += n_events;
                            }
                        }
                    }

                    Ok(total_events)
                }.await;

                __result
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
