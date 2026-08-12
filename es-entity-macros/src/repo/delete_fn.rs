use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::{
    error_classifier::write_error_classifier,
    events_write::{EventSource, EventsInsert, ForgettablePayloads},
    options::*,
};

pub struct DeleteFn<'a> {
    id: &'a syn::Ident,
    event: &'a syn::Ident,
    modify_error: syn::Ident,
    entity: &'a syn::Ident,
    table_name: &'a str,
    events_table_name: &'a str,
    event_ctx: bool,
    columns: &'a Columns,
    delete_option: &'a DeleteOption,
    nested_delete_fn_names: Vec<syn::Ident>,
    post_persist_error: Option<&'a syn::Type>,
    forgettable_table_name: Option<&'a str>,
    #[cfg(feature = "instrument")]
    repo_name_snake: String,
}

impl<'a> DeleteFn<'a> {
    pub fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            id: opts.id(),
            event: opts.event(),
            entity: opts.entity(),
            modify_error: opts.modify_error(),
            columns: &opts.columns,
            table_name: opts.table_name(),
            events_table_name: opts.events_table_name(),
            event_ctx: opts.event_context_enabled(),
            delete_option: &opts.delete,
            nested_delete_fn_names: opts
                .all_nested()
                .map(|f| f.delete_nested_fn_name())
                .collect(),
            post_persist_error: opts.post_persist_hook.as_ref().map(|h| &h.error),
            forgettable_table_name: opts.forgettable_table_name(),
            #[cfg(feature = "instrument")]
            repo_name_snake: opts.repo_name_snake_case(),
        }
    }
}

impl ToTokens for DeleteFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        if !self.delete_option.is_soft() {
            return;
        }

        let entity = self.entity;
        let modify_error = &self.modify_error;

        let nested_deletes = self.nested_delete_fn_names.iter().map(|f| {
            quote! {
                Self::#f::<_, _, #modify_error>(op, &entity).await?;
            }
        });

        // Soft-delete auto-forgets: forgettable index columns are set to NULL
        // (not re-persisted from the live entity), matching `forget()`.
        let assignments = self
            .columns
            .variable_assignments_for_delete(syn::parse_quote! { entity });
        let column_updates = self.columns.sql_updates_for_delete();
        let args = self.columns.update_query_args_for_delete();

        // The soft-delete flag and the entity's staged events go out as one
        // statement; the events insert reads from the CTE so the index write
        // is ordered first. `delete` may have no staged events at all, in
        // which case the UNNEST is empty and only the CTE does work.
        let events_insert = EventsInsert::new(self.events_table_name, self.event_ctx);
        let now_p = args.len() + 1;
        let source = EventSource::PerEntityCte {
            cte: "updated",
            offset_param: Some(now_p + 1),
        };
        let query = format!(
            "WITH updated AS (UPDATE {} SET {}{}deleted = TRUE WHERE id = $1 RETURNING id) {}",
            self.table_name,
            column_updates,
            if column_updates.is_empty() { "" } else { ", " },
            events_insert.sql(&source, now_p, now_p + 2),
        );

        let classifier = write_error_classifier(modify_error, self.events_table_name);
        let gather = events_insert.gather_per_entity(quote! { entity.events() });
        let event_args = events_insert.arg_exprs(&source);

        #[cfg(feature = "instrument")]
        let (instrument_attr, record_id, error_recording) = {
            let entity_name = entity.to_string();
            let repo_name = &self.repo_name_snake;
            let span_name = format!("{}.delete", repo_name);
            (
                quote! {
                    #[tracing::instrument(name = #span_name, skip_all, fields(entity = #entity_name, id = tracing::field::Empty, error = tracing::field::Empty, exception.message = tracing::field::Empty, exception.type = tracing::field::Empty))]
                },
                quote! {
                    tracing::Span::current().record("id", tracing::field::debug(&entity.id));
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

        let id_type = self.id;
        let event_type = self.event;
        // The payload purge runs *before* the combined statement: staged
        // events are persisted after it, so — as before — a payload staged but
        // not yet persisted at delete time survives the delete.
        let forget_payloads = if let Some(forgettable_tbl) = self.forgettable_table_name {
            let forget_query = format!("DELETE FROM {} WHERE entity_id = $1", forgettable_tbl);
            quote! {
                sqlx::query!(
                    #forget_query,
                    id as &#id_type
                )
                .execute(op.as_executor())
                .await?;
            }
        } else {
            quote! {}
        };

        let staged_payload_insert = match self.forgettable_table_name {
            Some(table) => ForgettablePayloads {
                table,
                id_type,
                event_type,
            }
            .insert_per_entity(quote! { entity.events() }, modify_error),
            None => quote! {},
        };

        tokens.append_all(quote! {
            pub async fn delete(
                &self,
                entity: #entity
            ) -> Result<(), #modify_error> {
                let mut op = self.begin_op().await?;
                let res = self.delete_in_op(&mut op, entity).await?;
                op.commit().await?;
                Ok(res)
            }

            #instrument_attr
            pub async fn delete_in_op<OP>(&self,
                op: &mut OP,
                mut entity: #entity
            ) -> Result<(), #modify_error>
            where
                OP: es_entity::AtomicOperation
            {
                let __result: Result<(), #modify_error> = async {
                    #(#nested_deletes)*
                    #assignments
                    #record_id

                    #forget_payloads

                    let new_events = entity.events().any_new();
                    #gather

                    let rows = sqlx::query!(
                        #query,
                        #(#args,)*
                        #(#event_args),*
                    )
                        .fetch_all(op.as_executor())
                        .await
                        .map_err(#classifier)?;

                    #staged_payload_insert

                    if new_events {
                        // No row means the CTE's UPDATE matched nothing — the
                        // entity was hard-deleted underneath us. That is a lost
                        // race, not an internal error.
                        let recorded_at = rows
                            .first()
                            .map(|row| row.recorded_at)
                            .ok_or(#modify_error::ConcurrentModification)?;
                        let n_events = Self::extract_events(&mut entity)
                            .mark_new_events_persisted_at(recorded_at);

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
    use super::*;
    use proc_macro2::Span;
    use syn::Ident;

    #[test]
    fn delete_fn() {
        let id = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let mut columns = Columns::default();
        columns.set_id_column(&id);

        let event = Ident::new("EntityEvent", Span::call_site());
        let delete_fn = DeleteFn {
            id: &id,
            event: &event,
            entity: &entity,
            modify_error: syn::Ident::new("EntityModifyError", Span::call_site()),
            table_name: "entities",
            events_table_name: "entity_events",
            event_ctx: false,
            columns: &columns,
            delete_option: &DeleteOption::Soft,
            nested_delete_fn_names: Vec::new(),
            post_persist_error: None,
            forgettable_table_name: None,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        delete_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn delete(
                &self,
                entity: Entity
            ) -> Result<(), EntityModifyError> {
                let mut op = self.begin_op().await?;
                let res = self.delete_in_op(&mut op, entity).await?;
                op.commit().await?;
                Ok(res)
            }

            pub async fn delete_in_op<OP>(
                &self,
                op: &mut OP,
                mut entity: Entity
            ) -> Result<(), EntityModifyError>
            where
                OP: es_entity::AtomicOperation
            {
                let __result: Result<(), EntityModifyError> = async {
                    let id = &entity.id;

                    let new_events = entity.events().any_new();
                    let offset = entity.events().len_persisted();
                    let events_types = entity.events().new_event_types();
                    let serialized_events = entity.events().serialize_new_events();

                    let rows = sqlx::query!(
                        "WITH updated AS (UPDATE entities SET deleted = TRUE WHERE id = $1 RETURNING id) INSERT INTO entity_events (id, recorded_at, sequence, event_type, event) SELECT updated.id, COALESCE($2, NOW()), ROW_NUMBER() OVER () + $3, unnested.event_type, unnested.event FROM updated CROSS JOIN UNNEST($4::TEXT[], $5::JSONB[]) AS unnested(event_type, event) RETURNING recorded_at",
                        id as &EntityId,
                        op.maybe_now(),
                        offset as i32,
                        &events_types,
                        &serialized_events
                    )
                        .fetch_all(op.as_executor())
                        .await
                        .map_err(|e| match &e {
                            sqlx::Error::Database(db_err)
                                if db_err.is_unique_violation()
                                    && db_err.table() == Some("entity_events") =>
                            {
                                EntityModifyError::ConcurrentModification
                            }
                            sqlx::Error::Database(db_err)
                                if db_err.table() != Some("entity_events")
                                    && es_entity::is_classified_constraint_violation(db_err.as_ref()) =>
                            {
                                EntityModifyError::ConstraintViolation {
                                    column: Self::map_constraint_column(db_err.constraint()),
                                    value: es_entity::extract_constraint_value(db_err.as_ref()),
                                    inner: e,
                                }
                            }
                            _ => EntityModifyError::Sqlx(e),
                        })?;

                    if new_events {
                        let recorded_at = rows
                            .first()
                            .map(|row| row.recorded_at)
                            .ok_or(EntityModifyError::ConcurrentModification)?;
                        let n_events = Self::extract_events(&mut entity)
                            .mark_new_events_persisted_at(recorded_at);
                    }

                    Ok(())
                }.await;

                __result
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn delete_fn_with_update_columns() {
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
        let delete_fn = DeleteFn {
            id: &id,
            event: &event,
            entity: &entity,
            modify_error: syn::Ident::new("EntityModifyError", Span::call_site()),
            table_name: "entities",
            events_table_name: "entity_events",
            event_ctx: false,
            columns: &columns,
            delete_option: &DeleteOption::Soft,
            nested_delete_fn_names: Vec::new(),
            post_persist_error: None,
            forgettable_table_name: None,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        delete_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn delete(
                &self,
                entity: Entity
            ) -> Result<(), EntityModifyError> {
                let mut op = self.begin_op().await?;
                let res = self.delete_in_op(&mut op, entity).await?;
                op.commit().await?;
                Ok(res)
            }

            pub async fn delete_in_op<OP>(
                &self,
                op: &mut OP,
                mut entity: Entity
            ) -> Result<(), EntityModifyError>
            where
                OP: es_entity::AtomicOperation
            {
                let __result: Result<(), EntityModifyError> = async {
                    let id = &entity.id;
                    let name = &entity.name;

                    let new_events = entity.events().any_new();
                    let offset = entity.events().len_persisted();
                    let events_types = entity.events().new_event_types();
                    let serialized_events = entity.events().serialize_new_events();

                    let rows = sqlx::query!(
                        "WITH updated AS (UPDATE entities SET name = $2, deleted = TRUE WHERE id = $1 RETURNING id) INSERT INTO entity_events (id, recorded_at, sequence, event_type, event) SELECT updated.id, COALESCE($3, NOW()), ROW_NUMBER() OVER () + $4, unnested.event_type, unnested.event FROM updated CROSS JOIN UNNEST($5::TEXT[], $6::JSONB[]) AS unnested(event_type, event) RETURNING recorded_at",
                        id as &EntityId,
                        name as &String,
                        op.maybe_now(),
                        offset as i32,
                        &events_types,
                        &serialized_events
                    )
                        .fetch_all(op.as_executor())
                        .await
                        .map_err(|e| match &e {
                            sqlx::Error::Database(db_err)
                                if db_err.is_unique_violation()
                                    && db_err.table() == Some("entity_events") =>
                            {
                                EntityModifyError::ConcurrentModification
                            }
                            sqlx::Error::Database(db_err)
                                if db_err.table() != Some("entity_events")
                                    && es_entity::is_classified_constraint_violation(db_err.as_ref()) =>
                            {
                                EntityModifyError::ConstraintViolation {
                                    column: Self::map_constraint_column(db_err.constraint()),
                                    value: es_entity::extract_constraint_value(db_err.as_ref()),
                                    inner: e,
                                }
                            }
                            _ => EntityModifyError::Sqlx(e),
                        })?;

                    if new_events {
                        let recorded_at = rows
                            .first()
                            .map(|row| row.recorded_at)
                            .ok_or(EntityModifyError::ConcurrentModification)?;
                        let n_events = Self::extract_events(&mut entity)
                            .mark_new_events_persisted_at(recorded_at);
                    }

                    Ok(())
                }.await;

                __result
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn delete_fn_with_forgettable() {
        let id = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let mut columns = Columns::default();
        columns.set_id_column(&id);

        let event = Ident::new("EntityEvent", Span::call_site());
        let delete_fn = DeleteFn {
            id: &id,
            event: &event,
            entity: &entity,
            modify_error: syn::Ident::new("EntityModifyError", Span::call_site()),
            table_name: "entities",
            events_table_name: "entity_events",
            event_ctx: false,
            columns: &columns,
            delete_option: &DeleteOption::Soft,
            nested_delete_fn_names: Vec::new(),
            post_persist_error: None,
            forgettable_table_name: Some("entities_forgettable_payloads"),
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        delete_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn delete(
                &self,
                entity: Entity
            ) -> Result<(), EntityModifyError> {
                let mut op = self.begin_op().await?;
                let res = self.delete_in_op(&mut op, entity).await?;
                op.commit().await?;
                Ok(res)
            }

            pub async fn delete_in_op<OP>(
                &self,
                op: &mut OP,
                mut entity: Entity
            ) -> Result<(), EntityModifyError>
            where
                OP: es_entity::AtomicOperation
            {
                let __result: Result<(), EntityModifyError> = async {
                    let id = &entity.id;

                    sqlx::query!(
                        "DELETE FROM entities_forgettable_payloads WHERE entity_id = $1",
                        id as &EntityId
                    )
                    .execute(op.as_executor())
                    .await?;

                    let new_events = entity.events().any_new();
                    let offset = entity.events().len_persisted();
                    let events_types = entity.events().new_event_types();
                    let serialized_events = entity.events().serialize_new_events();

                    let rows = sqlx::query!(
                        "WITH updated AS (UPDATE entities SET deleted = TRUE WHERE id = $1 RETURNING id) INSERT INTO entity_events (id, recorded_at, sequence, event_type, event) SELECT updated.id, COALESCE($2, NOW()), ROW_NUMBER() OVER () + $3, unnested.event_type, unnested.event FROM updated CROSS JOIN UNNEST($4::TEXT[], $5::JSONB[]) AS unnested(event_type, event) RETURNING recorded_at",
                        id as &EntityId,
                        op.maybe_now(),
                        offset as i32,
                        &events_types,
                        &serialized_events
                    )
                        .fetch_all(op.as_executor())
                        .await
                        .map_err(|e| match &e {
                            sqlx::Error::Database(db_err)
                                if db_err.is_unique_violation()
                                    && db_err.table() == Some("entity_events") =>
                            {
                                EntityModifyError::ConcurrentModification
                            }
                            sqlx::Error::Database(db_err)
                                if db_err.table() != Some("entity_events")
                                    && es_entity::is_classified_constraint_violation(db_err.as_ref()) =>
                            {
                                EntityModifyError::ConstraintViolation {
                                    column: Self::map_constraint_column(db_err.constraint()),
                                    value: es_entity::extract_constraint_value(db_err.as_ref()),
                                    inner: e,
                                }
                            }
                            _ => EntityModifyError::Sqlx(e),
                        })?;

                    let mut payload_sequences: Vec<i32> = Vec::new();
                    let mut payload_values: Vec<es_entity::prelude::serde_json::Value> = Vec::new();
                    for (idx, event_with_ctx) in entity.events().iter_new_events().enumerate() {
                        if let Some(payload) = EntityEvent::extract_forgettable_payloads(&event_with_ctx.event) {
                            payload_sequences.push((offset + 1 + idx) as i32);
                            payload_values.push(payload);
                        }
                    }
                    if !payload_sequences.is_empty() {
                        Self::extract_concurrent_modification(
                            sqlx::query!(
                                "INSERT INTO entities_forgettable_payloads (entity_id, sequence, payload) SELECT $1, unnested.sequence, unnested.payload FROM UNNEST($2::INT[], $3::JSONB[]) AS unnested(sequence, payload)",
                                id as &EntityId,
                                &payload_sequences,
                                &payload_values,
                            )
                            .execute(op.as_executor())
                            .await,
                            EntityModifyError::ConcurrentModification,
                        )?;
                    }

                    if new_events {
                        let recorded_at = rows
                            .first()
                            .map(|row| row.recorded_at)
                            .ok_or(EntityModifyError::ConcurrentModification)?;
                        let n_events = Self::extract_events(&mut entity)
                            .mark_new_events_persisted_at(recorded_at);
                    }

                    Ok(())
                }.await;

                __result
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
