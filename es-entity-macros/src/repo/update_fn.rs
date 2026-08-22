use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::{
    events_write::{EventSource, EventsInsert, ForgettablePayloads},
    options::*,
};

pub struct UpdateFn<'a> {
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

impl<'a> From<&'a RepositoryOptions> for UpdateFn<'a> {
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

impl ToTokens for UpdateFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let entity = self.entity;
        let modify_error = &self.modify_error;

        let nested = self.nested_fn_names.iter().map(|f| {
            quote! {
                self.#f(op, &mut [&mut *entity]).await?;
            }
        });

        // With index columns to persist, the index update and the event insert
        // are a single statement: the event insert reads from the `updated`
        // CTE, which forces the index write to happen first (preserving the
        // index-error-wins precedence of the previous two-statement form) and
        // supplies the id without re-binding it. Without index columns there
        // is nothing to combine, so the shared `persist_events` is used.
        let persist_tokens = if self.columns.updates_needed() {
            let assignments = self
                .columns
                .variable_assignments_for_update(syn::parse_quote! { entity });
            let column_updates = self.columns.sql_updates();
            let args = self.columns.update_query_args();

            let events_insert = EventsInsert::new(self.events_table_name, self.event_ctx);
            let now_p = args.len() + 1;
            let source = EventSource::PerEntityCte {
                cte: "updated",
                offset_param: Some(now_p + 1),
            };
            let query = format!(
                "WITH updated AS (UPDATE {} SET {} WHERE id = $1 RETURNING id) {}",
                self.table_name,
                column_updates,
                events_insert.sql(&source, now_p, now_p + 2),
            );

            let gather = events_insert.gather_per_entity(quote! { entity.events() });
            let event_args = events_insert.arg_exprs(&source);

            // Forgettable payloads are written to their own table, so they
            // stay a follow-up statement (still one fewer round trip than
            // before, where the index update was separate too).
            let forgettable_code = match self.forgettable_table_name {
                Some(table) => ForgettablePayloads {
                    table,
                    id_type: self.id,
                    event_type: self.event,
                }
                .insert_per_entity(quote! { entity.events() }, modify_error),
                None => quote! {},
            };

            quote! {
                #assignments
                #gather

                let rows = sqlx::query!(
                    #query,
                    #(#args,)*
                    #(#event_args),*
                )
                    .fetch_all(op.as_executor())
                    .await
                    .map_err(Self::classify_write_error)?;

                #forgettable_code

                // There are new events (checked above), so the events insert
                // produced a row unless the CTE's UPDATE matched nothing —
                // i.e. the index row was deleted underneath us. Report that as
                // a lost race, matching `update_all`, rather than leaking it as
                // an internal `Sqlx(RowNotFound)`.
                let recorded_at = rows
                    .first()
                    .map(|row| row.recorded_at)
                    .ok_or(#modify_error::ConcurrentModification)?;
                let n_events = Self::extract_events(entity).mark_new_events_persisted_at(recorded_at);
            }
        } else {
            quote! {
                let n_events = {
                    let events = Self::extract_events(entity);
                    Self::extract_concurrent_modification(
                        self.persist_events(op, events).await,
                        #modify_error::ConcurrentModification,
                    )?
                };
            }
        };

        #[cfg(feature = "instrument")]
        let (instrument_attr, record_id, error_recording) = {
            use convert_case::{Case, Casing};

            let entity_name = entity.to_string();
            let repo_name = &self.repo_name_snake;

            let id_ident = quote::format_ident!("{}_id", entity.to_string().to_case(Case::Snake));

            let span_name = format!("{}.update", repo_name);
            (
                quote! {
                    #[tracing::instrument(name = #span_name, skip_all, fields(entity = #entity_name, #id_ident = tracing::field::Empty, error = tracing::field::Empty, exception.message = tracing::field::Empty, exception.type = tracing::field::Empty))]
                },
                quote! {
                    tracing::Span::current().record(stringify!(#id_ident), tracing::field::display(&entity.id));
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
        let (instrument_attr, record_id, error_recording) = (quote! {}, quote! {}, quote! {});

        let post_persist_check = if self.post_persist_error.is_some() {
            quote! {
                self.execute_post_persist_hook(op, &entity, entity.events().last_persisted(n_events)).await.map_err(#modify_error::PostPersistHookError)?;
            }
        } else {
            quote! {}
        };

        tokens.append_all(quote! {
            #[inline(always)]
            fn extract_events<Entity, Event>(entity: &mut Entity) -> &mut es_entity::EntityEvents<Event>
            where
                Entity: es_entity::EsEntity<Event = Event>,
                Event: es_entity::EsEvent,
            {
                entity.events_mut()
            }

            pub async fn update(
                &self,
                entity: &mut #entity
            ) -> Result<usize, #modify_error> {
                let mut op = self.begin_op().await?;
                let res = self.update_in_op(&mut op, entity).await?;
                op.commit().await?;
                Ok(res)
            }

            #instrument_attr
            pub async fn update_in_op<OP>(
                &self,
                op: &mut OP,
                entity: &mut #entity
            ) -> Result<usize, #modify_error>
            where
                OP: es_entity::AtomicOperation
            {
                let __result: Result<usize, #modify_error> = async {
                    #record_id
                    #(#nested)*

                    if !Self::extract_events(entity).any_new() {
                        return Ok(0);
                    }

                    #persist_tokens

                    #post_persist_check

                    Ok(n_events)
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
    fn update_fn() {
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
        let update_fn = UpdateFn {
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
        update_fn.to_tokens(&mut tokens);

        let expected = quote! {
            #[inline(always)]
            fn extract_events<Entity, Event>(entity: &mut Entity) -> &mut es_entity::EntityEvents<Event>
            where
                Entity: es_entity::EsEntity<Event = Event>,
                Event: es_entity::EsEvent,
            {
                entity.events_mut()
            }

            pub async fn update(
                &self,
                entity: &mut Entity
            ) -> Result<usize, EntityModifyError> {
                let mut op = self.begin_op().await?;
                let res = self.update_in_op(&mut op, entity).await?;
                op.commit().await?;
                Ok(res)
            }

            pub async fn update_in_op<OP>(
                &self,
                op: &mut OP,
                entity: &mut Entity
            ) -> Result<usize, EntityModifyError>
            where
                OP: es_entity::AtomicOperation
            {
                let __result: Result<usize, EntityModifyError> = async {
                    if !Self::extract_events(entity).any_new() {
                        return Ok(0);
                    }

                    let id = &entity.id;
                    let name = &entity.name;
                    let offset = entity.events().len_persisted();
                    let events_types = entity.events().new_event_types();
                    let serialized_events = entity.events().serialize_new_events();

                    let rows = sqlx::query!(
                        "WITH updated AS (UPDATE entities SET name = $2 WHERE id = $1 RETURNING id) INSERT INTO entity_events (id, recorded_at, sequence, event_type, event) SELECT updated.id, COALESCE($3, NOW()), ROW_NUMBER() OVER () + $4, unnested.event_type, unnested.event FROM updated CROSS JOIN UNNEST($5::TEXT[], $6::JSONB[]) AS unnested(event_type, event) RETURNING recorded_at",
                        id as &EntityId,
                        name as &String,
                        op.maybe_now(),
                        offset as i32,
                        &events_types,
                        &serialized_events
                    )
                        .fetch_all(op.as_executor())
                        .await
                        .map_err(Self::classify_write_error)?;

                    let recorded_at = rows
                        .first()
                        .map(|row| row.recorded_at)
                        .ok_or(EntityModifyError::ConcurrentModification)?;
                    let n_events = Self::extract_events(entity).mark_new_events_persisted_at(recorded_at);

                    Ok(n_events)
                }.await;

                __result
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn update_fn_no_columns() {
        let id = syn::parse_str("EntityId").unwrap();
        let entity = Ident::new("Entity", Span::call_site());

        let mut columns = Columns::default();
        columns.set_id_column(&id);

        let event = Ident::new("EntityEvent", Span::call_site());
        let update_fn = UpdateFn {
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
        update_fn.to_tokens(&mut tokens);

        let expected = quote! {
            #[inline(always)]
            fn extract_events<Entity, Event>(entity: &mut Entity) -> &mut es_entity::EntityEvents<Event>
            where
                Entity: es_entity::EsEntity<Event = Event>,
                Event: es_entity::EsEvent,
            {
                entity.events_mut()
            }

            pub async fn update(
                &self,
                entity: &mut Entity
            ) -> Result<usize, EntityModifyError> {
                let mut op = self.begin_op().await?;
                let res = self.update_in_op(&mut op, entity).await?;
                op.commit().await?;
                Ok(res)
            }

            pub async fn update_in_op<OP>(
                &self,
                op: &mut OP,
                entity: &mut Entity
            ) -> Result<usize, EntityModifyError>
            where
                OP: es_entity::AtomicOperation
            {
                let __result: Result<usize, EntityModifyError> = async {
                    if !Self::extract_events(entity).any_new() {
                        return Ok(0);
                    }

                    let n_events = {
                        let events = Self::extract_events(entity);
                        Self::extract_concurrent_modification(
                            self.persist_events(op, events).await,
                            EntityModifyError::ConcurrentModification,
                        )?
                    };

                    Ok(n_events)
                }.await;

                __result
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
