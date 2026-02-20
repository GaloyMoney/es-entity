use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::*;

pub struct UpdateAllFn<'a> {
    entity: &'a syn::Ident,
    id: &'a syn::Ident,
    table_name: &'a str,
    events_table_name: &'a str,
    columns: &'a Columns,
    error: &'a syn::Type,
    event_ctx: bool,
    nested_fn_names: Vec<syn::Ident>,
    #[cfg(feature = "instrument")]
    repo_name_snake: String,
}

impl<'a> From<&'a RepositoryOptions> for UpdateAllFn<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            entity: opts.entity(),
            id: opts.id(),
            error: opts.err(),
            columns: &opts.columns,
            table_name: opts.table_name(),
            events_table_name: opts.events_table_name(),
            event_ctx: opts.event_context_enabled(),
            nested_fn_names: opts
                .all_nested()
                .map(|f| f.update_nested_fn_name())
                .collect(),
            #[cfg(feature = "instrument")]
            repo_name_snake: opts.repo_name_snake_case(),
        }
    }
}

impl ToTokens for UpdateAllFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let entity = self.entity;
        let id_type = self.id;
        let error = self.error;

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

        let (vec_declarations, per_entity_pushes, update_tokens) = if self.columns.updates_needed()
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
            let query = format!(
                "UPDATE {table_name} SET {set_clause} \
                     FROM UNNEST({placeholders}) \
                     AS unnested({column_list}) \
                     WHERE {table_name}.id = unnested.id",
            );
            (
                Some(vecs),
                Some(pushes),
                Some(quote! {
                    sqlx::query(#query)
                        #(#bind_tokens)*
                        .execute(op.as_executor())
                        .await?;
                }),
            )
        } else {
            (None, None, None)
        };

        let events_query = format!(
            "INSERT INTO {} (id, recorded_at, sequence, event_type, event{}) \
             SELECT unnested.id, COALESCE($1, NOW()), unnested.sequence, unnested.event_type, unnested.event{} \
             FROM UNNEST($2, $3::INT[], $4::TEXT[], $5::JSONB[]{}) \
             AS unnested(id, sequence, event_type, event{}) RETURNING recorded_at",
            self.events_table_name,
            if self.event_ctx { ", context" } else { "" },
            if self.event_ctx {
                ", unnested.context"
            } else {
                ""
            },
            if self.event_ctx { ", $6::JSONB[]" } else { "" },
            if self.event_ctx { ", context" } else { "" }
        );

        let (ctx_var, ctx_extend, ctx_bind) = if self.event_ctx {
            (
                quote! {
                    let mut all_contexts: Vec<es_entity::ContextData> = Vec::new();
                },
                quote! {
                    let contexts = entity.events().serialize_new_event_contexts();
                    if let Some(contexts) = contexts {
                        all_contexts.extend(contexts);
                    }
                },
                quote! {
                    .bind(&if all_contexts.is_empty() {
                        None
                    } else {
                         Some(all_contexts)
                    })
                },
            )
        } else {
            (quote! {}, quote! {}, quote! {})
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

        tokens.append_all(quote! {
            pub async fn update_all(
                &self,
                entities: &mut [#entity]
            ) -> Result<usize, #error> {
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
            ) -> Result<usize, #error>
            where
                OP: es_entity::AtomicOperation
            {
                let __result: Result<usize, #error> = async {
                    if entities.is_empty() {
                        return Ok(0);
                    }

                    #nested_phase

                    #vec_declarations
                    let mut all_serialized = Vec::new();
                    #ctx_var
                    let mut all_types = Vec::new();
                    let mut all_ids: Vec<&#id_type> = Vec::new();
                    let mut all_sequences = Vec::new();
                    let mut n_events_map = std::collections::HashMap::new();

                    for entity in entities.iter() {
                        if !entity.events().any_new() {
                            continue;
                        }

                        #per_entity_pushes

                        {
                            let events = entity.events();
                            let id = events.id();
                            let offset = events.len_persisted() + 1;
                            let serialized = events.serialize_new_events();
                            #ctx_extend
                            let types = serialized.iter()
                                .map(|e| e.get("type")
                                    .and_then(es_entity::prelude::serde_json::Value::as_str)
                                    .expect("Could not read event type")
                                    .to_owned())
                                .collect::<Vec<_>>();

                            let n_events = serialized.len();
                            all_serialized.extend(serialized);
                            all_types.extend(types);
                            all_ids.extend(std::iter::repeat(id).take(n_events));
                            all_sequences.extend((offset..).take(n_events).map(|i| i as i32));
                            n_events_map.insert(id.clone(), n_events);
                        }
                    }

                    if n_events_map.is_empty() {
                        return Ok(0);
                    }

                    #update_tokens

                    let now = op.maybe_now();
                    let rows = Self::extract_concurrent_modification(
                        sqlx::query(#events_query)
                            .bind(now)
                            .bind(&all_ids)
                            .bind(&all_sequences)
                            .bind(&all_types)
                            .bind(&all_serialized)
                            #ctx_bind
                            .fetch_all(op.as_executor())
                            .await
                    )?;

                    let recorded_at: chrono::DateTime<chrono::Utc> = {
                        use es_entity::prelude::sqlx::Row;
                        rows[0].try_get("recorded_at").expect("no recorded at")
                    };

                    let mut total_events = 0usize;
                    for entity in entities.iter_mut() {
                        let n_events = Self::extract_events(entity).mark_new_events_persisted_at(recorded_at);
                        if n_events > 0 {
                            self.execute_post_persist_hook(op, &entity, entity.events().last_persisted(n_events)).await?;
                            total_events += n_events;
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
        let error = syn::parse_str("es_entity::EsRepoError").unwrap();

        let columns = Columns::new(
            &id,
            [Column::new(
                Ident::new("name", Span::call_site()),
                syn::parse_str("String").unwrap(),
            )],
        );

        let update_all_fn = UpdateAllFn {
            entity: &entity,
            id: &id,
            table_name: "entities",
            events_table_name: "entity_events",
            error: &error,
            columns: &columns,
            event_ctx: false,
            nested_fn_names: Vec::new(),
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        update_all_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn update_all(
                &self,
                entities: &mut [Entity]
            ) -> Result<usize, es_entity::EsRepoError> {
                let mut op = self.begin_op().await?;
                let res = self.update_all_in_op(&mut op, entities).await?;
                op.commit().await?;
                Ok(res)
            }

            pub async fn update_all_in_op<OP>(
                &self,
                op: &mut OP,
                entities: &mut [Entity]
            ) -> Result<usize, es_entity::EsRepoError>
            where
                OP: es_entity::AtomicOperation
            {
                let __result: Result<usize, es_entity::EsRepoError> = async {
                    if entities.is_empty() {
                        return Ok(0);
                    }

                    let mut id_collection = Vec::new();
                    let mut name_collection = Vec::new();
                    let mut all_serialized = Vec::new();
                    let mut all_types = Vec::new();
                    let mut all_ids: Vec<&EntityId> = Vec::new();
                    let mut all_sequences = Vec::new();
                    let mut n_events_map = std::collections::HashMap::new();

                    for entity in entities.iter() {
                        if !entity.events().any_new() {
                            continue;
                        }

                        let id = &entity.id;
                        let name = &entity.name;
                        id_collection.push(id);
                        name_collection.push(name);

                        {
                            let events = entity.events();
                            let id = events.id();
                            let offset = events.len_persisted() + 1;
                            let serialized = events.serialize_new_events();
                            let types = serialized.iter()
                                .map(|e| e.get("type")
                                    .and_then(es_entity::prelude::serde_json::Value::as_str)
                                    .expect("Could not read event type")
                                    .to_owned())
                                .collect::<Vec<_>>();

                            let n_events = serialized.len();
                            all_serialized.extend(serialized);
                            all_types.extend(types);
                            all_ids.extend(std::iter::repeat(id).take(n_events));
                            all_sequences.extend((offset..).take(n_events).map(|i| i as i32));
                            n_events_map.insert(id.clone(), n_events);
                        }
                    }

                    if n_events_map.is_empty() {
                        return Ok(0);
                    }

                    sqlx::query("UPDATE entities SET name = unnested.name FROM UNNEST($1, $2) AS unnested(id, name) WHERE entities.id = unnested.id")
                        .bind(id_collection)
                        .bind(name_collection)
                        .execute(op.as_executor())
                        .await?;

                    let now = op.maybe_now();
                    let rows = Self::extract_concurrent_modification(
                        sqlx::query("INSERT INTO entity_events (id, recorded_at, sequence, event_type, event) SELECT unnested.id, COALESCE($1, NOW()), unnested.sequence, unnested.event_type, unnested.event FROM UNNEST($2, $3::INT[], $4::TEXT[], $5::JSONB[]) AS unnested(id, sequence, event_type, event) RETURNING recorded_at")
                            .bind(now)
                            .bind(&all_ids)
                            .bind(&all_sequences)
                            .bind(&all_types)
                            .bind(&all_serialized)
                            .fetch_all(op.as_executor())
                            .await
                    )?;

                    let recorded_at: chrono::DateTime<chrono::Utc> = {
                        use es_entity::prelude::sqlx::Row;
                        rows[0].try_get("recorded_at").expect("no recorded at")
                    };

                    let mut total_events = 0usize;
                    for entity in entities.iter_mut() {
                        let n_events = Self::extract_events(entity).mark_new_events_persisted_at(recorded_at);
                        if n_events > 0 {
                            self.execute_post_persist_hook(op, &entity, entity.events().last_persisted(n_events)).await?;
                            total_events += n_events;
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
        let error = syn::parse_str("es_entity::EsRepoError").unwrap();

        let mut columns = Columns::default();
        columns.set_id_column(&id);

        let update_all_fn = UpdateAllFn {
            entity: &entity,
            id: &id,
            table_name: "entities",
            events_table_name: "entity_events",
            error: &error,
            columns: &columns,
            event_ctx: false,
            nested_fn_names: Vec::new(),
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        update_all_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn update_all(
                &self,
                entities: &mut [Entity]
            ) -> Result<usize, es_entity::EsRepoError> {
                let mut op = self.begin_op().await?;
                let res = self.update_all_in_op(&mut op, entities).await?;
                op.commit().await?;
                Ok(res)
            }

            pub async fn update_all_in_op<OP>(
                &self,
                op: &mut OP,
                entities: &mut [Entity]
            ) -> Result<usize, es_entity::EsRepoError>
            where
                OP: es_entity::AtomicOperation
            {
                let __result: Result<usize, es_entity::EsRepoError> = async {
                    if entities.is_empty() {
                        return Ok(0);
                    }

                    let mut all_serialized = Vec::new();
                    let mut all_types = Vec::new();
                    let mut all_ids: Vec<&EntityId> = Vec::new();
                    let mut all_sequences = Vec::new();
                    let mut n_events_map = std::collections::HashMap::new();

                    for entity in entities.iter() {
                        if !entity.events().any_new() {
                            continue;
                        }

                        {
                            let events = entity.events();
                            let id = events.id();
                            let offset = events.len_persisted() + 1;
                            let serialized = events.serialize_new_events();
                            let types = serialized.iter()
                                .map(|e| e.get("type")
                                    .and_then(es_entity::prelude::serde_json::Value::as_str)
                                    .expect("Could not read event type")
                                    .to_owned())
                                .collect::<Vec<_>>();

                            let n_events = serialized.len();
                            all_serialized.extend(serialized);
                            all_types.extend(types);
                            all_ids.extend(std::iter::repeat(id).take(n_events));
                            all_sequences.extend((offset..).take(n_events).map(|i| i as i32));
                            n_events_map.insert(id.clone(), n_events);
                        }
                    }

                    if n_events_map.is_empty() {
                        return Ok(0);
                    }

                    let now = op.maybe_now();
                    let rows = Self::extract_concurrent_modification(
                        sqlx::query("INSERT INTO entity_events (id, recorded_at, sequence, event_type, event) SELECT unnested.id, COALESCE($1, NOW()), unnested.sequence, unnested.event_type, unnested.event FROM UNNEST($2, $3::INT[], $4::TEXT[], $5::JSONB[]) AS unnested(id, sequence, event_type, event) RETURNING recorded_at")
                            .bind(now)
                            .bind(&all_ids)
                            .bind(&all_sequences)
                            .bind(&all_types)
                            .bind(&all_serialized)
                            .fetch_all(op.as_executor())
                            .await
                    )?;

                    let recorded_at: chrono::DateTime<chrono::Utc> = {
                        use es_entity::prelude::sqlx::Row;
                        rows[0].try_get("recorded_at").expect("no recorded at")
                    };

                    let mut total_events = 0usize;
                    for entity in entities.iter_mut() {
                        let n_events = Self::extract_events(entity).mark_new_events_persisted_at(recorded_at);
                        if n_events > 0 {
                            self.execute_post_persist_hook(op, &entity, entity.events().last_persisted(n_events)).await?;
                            total_events += n_events;
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
