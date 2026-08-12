use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::{error_classifier::write_error_classifier, options::*};

pub struct CreateAllFn<'a> {
    entity: &'a syn::Ident,
    id: &'a syn::Ident,
    event: &'a syn::Ident,
    table_name: &'a str,
    events_table_name: &'a str,
    event_ctx: bool,
    forgettable_table_name: Option<&'a str>,
    columns: &'a Columns,
    create_error: syn::Ident,
    nested_fn_names: Vec<syn::Ident>,
    post_hydrate_error: Option<&'a syn::Type>,
    post_persist_error: Option<&'a syn::Type>,
    #[cfg(feature = "instrument")]
    repo_name_snake: String,
}

impl<'a> From<&'a RepositoryOptions> for CreateAllFn<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            table_name: opts.table_name(),
            entity: opts.entity(),
            id: opts.id(),
            event: opts.event(),
            events_table_name: opts.events_table_name(),
            event_ctx: opts.event_context_enabled(),
            forgettable_table_name: opts.forgettable_table_name(),
            create_error: opts.create_error(),
            nested_fn_names: opts
                .all_nested()
                .map(|f| f.create_nested_fn_name())
                .collect(),
            columns: &opts.columns,
            post_hydrate_error: opts.post_hydrate_hook.as_ref().map(|h| &h.error),
            post_persist_error: opts.post_persist_hook.as_ref().map(|h| &h.error),
            #[cfg(feature = "instrument")]
            repo_name_snake: opts.repo_name_snake_case(),
        }
    }
}

impl ToTokens for CreateAllFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let entity = self.entity;
        let create_error = &self.create_error;

        let nested = self.nested_fn_names.iter().map(|f| {
            quote! {
                self.#f(op, &mut entity).await?;
            }
        });
        let maybe_mut_entity = if self.nested_fn_names.is_empty() {
            quote! { entity }
        } else {
            quote! { mut entity }
        };

        let table_name = self.table_name;

        let column_names = self.columns.insert_column_names();
        let placeholders = self.columns.insert_placeholders(1);
        let (arg_collection, arg_adds) = self
            .columns
            .create_all_arg_collection(syn::parse_quote! { new_entity });

        let ids_p = column_names.len() + 2;
        let sequences_p = ids_p + 1;
        let types_p = ids_p + 2;
        let events_p = ids_p + 3;
        let ctx_p = ids_p + 4;

        // Both inserts in one statement. The events insert joins the `new_rows`
        // CTE, which forces the index rows to be written first — so a duplicate
        // id surfaces as the index constraint violation rather than as a unique
        // violation on the events table.
        let query = format!(
            "WITH new_rows AS (INSERT INTO {} (created_at, {}) \
            SELECT COALESCE($1, NOW()), unnested.{} \
            FROM UNNEST({}) \
            AS unnested({}) RETURNING id) \
            INSERT INTO {} (id, recorded_at, sequence, event_type, event{}) \
            SELECT unnested.id, COALESCE($1, NOW()), unnested.sequence, unnested.event_type, unnested.event{} \
            FROM UNNEST(${}, ${}::INT[], ${}::TEXT[], ${}::JSONB[]{}) \
            AS unnested(id, sequence, event_type, event{}) \
            JOIN new_rows ON new_rows.id = unnested.id \
            RETURNING recorded_at",
            table_name,
            column_names.join(", "),
            column_names.join(", unnested."),
            placeholders,
            column_names.join(", "),
            self.events_table_name,
            if self.event_ctx { ", context" } else { "" },
            if self.event_ctx {
                ", unnested.context"
            } else {
                ""
            },
            ids_p,
            sequences_p,
            types_p,
            events_p,
            if self.event_ctx {
                format!(", ${ctx_p}::JSONB[]")
            } else {
                String::new()
            },
            if self.event_ctx { ", context" } else { "" },
        );

        let classifier = write_error_classifier(create_error, self.events_table_name);

        let id_type = self.id;
        let event_type = self.event;

        let (ctx_var, ctx_extend, ctx_add) = if self.event_ctx {
            (
                quote! {
                    let mut all_contexts: Vec<es_entity::ContextData> = Vec::new();
                },
                quote! {
                    if let Some(contexts) = events.serialize_new_event_contexts() {
                        all_contexts.extend(contexts);
                    }
                },
                quote! {
                    __query_args.add(if all_contexts.is_empty() {
                        None
                    } else {
                        Some(&all_contexts)
                    }).map_err(sqlx::Error::Encode)?;
                },
            )
        } else {
            (quote! {}, quote! {}, quote! {})
        };

        let (forgettable_vars, forgettable_extract, forgettable_insert) = if let Some(
            forgettable_tbl,
        ) =
            self.forgettable_table_name
        {
            let payload_insert_query = format!(
                "INSERT INTO {} (entity_id, sequence, payload) SELECT unnested.entity_id, unnested.sequence, unnested.payload FROM UNNEST($1, $2::INT[], $3::JSONB[]) AS unnested(entity_id, sequence, payload)",
                forgettable_tbl
            );
            (
                quote! {
                    let mut payload_ids: Vec<&#id_type> = Vec::new();
                    let mut payload_sequences: Vec<i32> = Vec::new();
                    let mut payload_values: Vec<es_entity::prelude::serde_json::Value> = Vec::new();
                },
                quote! {
                    for (idx, event_with_ctx) in events.iter_new_events().enumerate() {
                        if let Some(payload) = #event_type::extract_forgettable_payloads(&event_with_ctx.event) {
                            payload_ids.push(id);
                            payload_sequences.push((offset + idx) as i32);
                            payload_values.push(payload);
                        }
                    }
                },
                quote! {
                    if !payload_sequences.is_empty() {
                        Self::extract_concurrent_modification(
                            sqlx::query(#payload_insert_query)
                                .bind(&payload_ids)
                                .bind(&payload_sequences)
                                .bind(&payload_values)
                                .execute(op.as_executor())
                                .await,
                            #create_error::ConcurrentModification,
                        )?;
                    }
                },
            )
        } else {
            (quote! {}, quote! {}, quote! {})
        };

        #[cfg(feature = "instrument")]
        let (instrument_attr, error_recording) = {
            let entity_name = entity.to_string();
            let repo_name = &self.repo_name_snake;
            let span_name = format!("{}.create_all", repo_name);
            (
                quote! {
                    #[tracing::instrument(name = #span_name, skip_all, fields(entity = #entity_name, count = new_entities.len(), error = tracing::field::Empty, exception.message = tracing::field::Empty, exception.type = tracing::field::Empty))]
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

        let post_hydrate_check = if self.post_hydrate_error.is_some() {
            quote! {
                self.execute_post_hydrate_hook(&entity).map_err(#create_error::PostHydrateError)?;
            }
        } else {
            quote! {}
        };

        let post_persist_check = if self.post_persist_error.is_some() {
            quote! {
                self.execute_post_persist_hook(op, &entity, entity.events().last_persisted(n_events)).await.map_err(#create_error::PostPersistHookError)?;
            }
        } else {
            quote! {}
        };

        tokens.append_all(quote! {
            pub async fn create_all(
                &self,
                new_entities: Vec<<#entity as es_entity::EsEntity>::New>
            ) -> Result<Vec<#entity>, #create_error> {
                let mut op = self.begin_op().await?;
                let res = self.create_all_in_op(&mut op, new_entities).await?;
                op.commit().await?;
                Ok(res)
            }

            #instrument_attr
            pub async fn create_all_in_op<OP>(
                &self,
                op: &mut OP,
                new_entities: Vec<<#entity as es_entity::EsEntity>::New>
            ) -> Result<Vec<#entity>, #create_error>
            where
                OP: es_entity::AtomicOperation
            {
                let __result: Result<Vec<#entity>, #create_error> = async {
                    use es_entity::prelude::sqlx::{Arguments, Row};

                    let mut res = Vec::new();
                    if new_entities.is_empty() {
                        return Ok(res);
                    }

                    let mut __query_args = sqlx::postgres::PgArguments::default();
                    __query_args.add(op.maybe_now()).map_err(sqlx::Error::Encode)?;

                    // The index columns are encoded first so that the borrows
                    // they hold on `new_entities` end here, freeing it to be
                    // consumed into the event stream below.
                    #arg_collection
                    #(#arg_adds)*

                    let mut all_events: Vec<es_entity::EntityEvents<<#entity as es_entity::EsEntity>::Event>> = new_entities.into_iter().map(Self::convert_new).collect();

                    let mut all_ids: Vec<&#id_type> = Vec::new();
                    let mut all_sequences: Vec<i32> = Vec::new();
                    let mut all_types = Vec::new();
                    let mut all_serialized = Vec::new();
                    let mut n_persisted: Vec<usize> = Vec::new();
                    #ctx_var
                    #forgettable_vars

                    for events in all_events.iter() {
                        let id = events.id();
                        let offset = events.len_persisted() + 1;
                        let types = events.new_event_types();
                        let serialized = events.serialize_new_events();
                        #ctx_extend
                        #forgettable_extract

                        let n_events = serialized.len();
                        all_types.extend(types);
                        all_serialized.extend(serialized);
                        all_ids.extend(std::iter::repeat(id).take(n_events));
                        all_sequences.extend((offset..).take(n_events).map(|i| i as i32));
                        n_persisted.push(n_events);
                    }

                    __query_args.add(&all_ids).map_err(sqlx::Error::Encode)?;
                    __query_args.add(&all_sequences).map_err(sqlx::Error::Encode)?;
                    __query_args.add(&all_types).map_err(sqlx::Error::Encode)?;
                    __query_args.add(&all_serialized).map_err(sqlx::Error::Encode)?;
                    #ctx_add

                    let expected_events = all_ids.len();
                    let rows = sqlx::query_with(#query, __query_args)
                        .fetch_all(op.as_executor())
                        .await
                        .map_err(#classifier)?;

                    #forgettable_insert

                    if expected_events > 0 {
                        // Every event row joins an index row this same statement
                        // inserted, so a short count means the join dropped rows.
                        if rows.len() != expected_events {
                            return Err(#create_error::ConcurrentModification);
                        }

                        let recorded_at = rows
                            .first()
                            .ok_or(sqlx::Error::RowNotFound)
                            .and_then(|row| row.try_get("recorded_at"))?;
                        for events in all_events.iter_mut() {
                            events.mark_new_events_persisted_at(recorded_at);
                        }
                    }

                    for (events, n_events) in all_events.into_iter().zip(n_persisted) {
                        let #maybe_mut_entity = Self::hydrate_entity(events)?;

                        #(#nested)*

                        #post_hydrate_check
                        #post_persist_check
                        res.push(entity);
                    }

                    Ok(res)
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
    fn create_all_fn() {
        let entity = Ident::new("Entity", Span::call_site());
        let create_error = syn::Ident::new("EntityCreateError", Span::call_site());
        let id = Ident::new("EntityId", Span::call_site());
        let event = Ident::new("EntityEvent", Span::call_site());

        use darling::FromMeta;
        let input: syn::Meta = syn::parse_quote!(columns(name = "String",));
        let mut columns = Columns::from_meta(&input).expect("Failed to parse Fields");
        columns.set_id_column(&id);

        let create_fn = CreateAllFn {
            table_name: "entities",
            entity: &entity,
            id: &id,
            event: &event,
            events_table_name: "entity_events",
            event_ctx: false,
            forgettable_table_name: None,
            create_error,
            columns: &columns,
            nested_fn_names: Vec::new(),
            post_hydrate_error: None,
            post_persist_error: None,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        create_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn create_all(
                &self,
                new_entities: Vec<<Entity as es_entity::EsEntity>::New>
            ) -> Result<Vec<Entity>, EntityCreateError> {
                let mut op = self.begin_op().await?;
                let res = self.create_all_in_op(&mut op, new_entities).await?;
                op.commit().await?;
                Ok(res)
            }

            pub async fn create_all_in_op<OP>(
                &self,
                op: &mut OP,
                new_entities: Vec<<Entity as es_entity::EsEntity>::New>
            ) -> Result<Vec<Entity>, EntityCreateError>
            where
                OP: es_entity::AtomicOperation
            {
                let __result: Result<Vec<Entity>, EntityCreateError> = async {
                    use es_entity::prelude::sqlx::{Arguments, Row};

                    let mut res = Vec::new();
                    if new_entities.is_empty() {
                        return Ok(res);
                    }

                    let mut __query_args = sqlx::postgres::PgArguments::default();
                    __query_args.add(op.maybe_now()).map_err(sqlx::Error::Encode)?;

                    let mut id_collection = Vec::new();
                    let mut name_collection = Vec::new();

                    for new_entity in new_entities.iter() {
                        let id: &EntityId = &new_entity.id;
                        let name: &String = &new_entity.name;

                        id_collection.push(id);
                        name_collection.push(name);
                    }

                    __query_args.add(id_collection).map_err(sqlx::Error::Encode)?;
                    __query_args.add(name_collection).map_err(sqlx::Error::Encode)?;

                    let mut all_events: Vec<es_entity::EntityEvents<<Entity as es_entity::EsEntity>::Event>> = new_entities.into_iter().map(Self::convert_new).collect();

                    let mut all_ids: Vec<&EntityId> = Vec::new();
                    let mut all_sequences: Vec<i32> = Vec::new();
                    let mut all_types = Vec::new();
                    let mut all_serialized = Vec::new();
                    let mut n_persisted: Vec<usize> = Vec::new();

                    for events in all_events.iter() {
                        let id = events.id();
                        let offset = events.len_persisted() + 1;
                        let types = events.new_event_types();
                        let serialized = events.serialize_new_events();

                        let n_events = serialized.len();
                        all_types.extend(types);
                        all_serialized.extend(serialized);
                        all_ids.extend(std::iter::repeat(id).take(n_events));
                        all_sequences.extend((offset..).take(n_events).map(|i| i as i32));
                        n_persisted.push(n_events);
                    }

                    __query_args.add(&all_ids).map_err(sqlx::Error::Encode)?;
                    __query_args.add(&all_sequences).map_err(sqlx::Error::Encode)?;
                    __query_args.add(&all_types).map_err(sqlx::Error::Encode)?;
                    __query_args.add(&all_serialized).map_err(sqlx::Error::Encode)?;

                    let expected_events = all_ids.len();
                    let rows = sqlx::query_with(
                        "WITH new_rows AS (INSERT INTO entities (created_at, id, name) SELECT COALESCE($1, NOW()), unnested.id, unnested.name FROM UNNEST($2, $3) AS unnested(id, name) RETURNING id) INSERT INTO entity_events (id, recorded_at, sequence, event_type, event) SELECT unnested.id, COALESCE($1, NOW()), unnested.sequence, unnested.event_type, unnested.event FROM UNNEST($4, $5::INT[], $6::TEXT[], $7::JSONB[]) AS unnested(id, sequence, event_type, event) JOIN new_rows ON new_rows.id = unnested.id RETURNING recorded_at",
                        __query_args
                    )
                        .fetch_all(op.as_executor())
                        .await
                        .map_err(|e| match &e {
                            sqlx::Error::Database(db_err)
                                if db_err.is_unique_violation()
                                    && db_err.table() == Some("entity_events") =>
                            {
                                EntityCreateError::ConcurrentModification
                            }
                            sqlx::Error::Database(db_err)
                                if db_err.table() != Some("entity_events")
                                    && es_entity::is_classified_constraint_violation(db_err.as_ref()) =>
                            {
                                EntityCreateError::ConstraintViolation {
                                    column: Self::map_constraint_column(db_err.constraint()),
                                    value: es_entity::extract_constraint_value(db_err.as_ref()),
                                    inner: e,
                                }
                            }
                            _ => EntityCreateError::Sqlx(e),
                        })?;

                    if expected_events > 0 {
                        if rows.len() != expected_events {
                            return Err(EntityCreateError::ConcurrentModification);
                        }

                        let recorded_at = rows
                            .first()
                            .ok_or(sqlx::Error::RowNotFound)
                            .and_then(|row| row.try_get("recorded_at"))?;
                        for events in all_events.iter_mut() {
                            events.mark_new_events_persisted_at(recorded_at);
                        }
                    }

                    for (events, n_events) in all_events.into_iter().zip(n_persisted) {
                        let entity = Self::hydrate_entity(events)?;

                        res.push(entity);
                    }

                    Ok(res)
                }.await;

                __result
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
