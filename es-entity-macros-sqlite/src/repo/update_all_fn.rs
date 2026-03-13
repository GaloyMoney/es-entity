use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::*;

pub struct UpdateAllFn<'a> {
    entity: &'a syn::Ident,
    table_name: &'a str,
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
            modify_error: opts.modify_error(),
            columns: &opts.columns,
            table_name: opts.table_name(),
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

        let update_tokens = if self.columns.updates_needed() {
            let assignments = self
                .columns
                .variable_assignments_for_update(syn::parse_quote! { entity });
            let column_updates = self.columns.sql_updates();
            let table_name = self.table_name;
            let query = format!("UPDATE {} SET {} WHERE id = ?1", table_name, column_updates,);
            let args = self.columns.update_query_args();

            Some(quote! {
                for entity in entities.iter() {
                    if !entity.events().any_new() {
                        continue;
                    }

                    #assignments
                    sqlx::query(#query)
                        #(#args)*
                        .execute(op.as_executor())
                        .await
                        .map_err(|e| match &e {
                            sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
                                #modify_error::ConstraintViolation {
                                    column: Self::map_constraint_column(db_err.constraint()),
                                    value: es_entity::db::extract_constraint_value(db_err.as_ref()),
                                    inner: e,
                                }
                            }
                            _ => #modify_error::Sqlx(e),
                        })?;
                }
            })
        } else {
            None
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
                    if entities.is_empty() {
                        return Ok(0);
                    }

                    #nested_phase

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

                    #update_tokens

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

        let update_all_fn = UpdateAllFn {
            entity: &entity,
            table_name: "entities",
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

                    for entity in entities.iter() {
                        if !entity.events().any_new() {
                            continue;
                        }

                        let id = &entity.id;
                        let name = &entity.name;
                        sqlx::query("UPDATE entities SET name = ?2 WHERE id = ?1")
                            .bind(id)
                            .bind(name)
                            .execute(op.as_executor())
                            .await
                            .map_err(|e| match &e {
                                sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
                                    EntityModifyError::ConstraintViolation {
                                        column: Self::map_constraint_column(db_err.constraint()),
                                        value: es_entity::db::extract_constraint_value(db_err.as_ref()),
                                        inner: e,
                                    }
                                }
                                _ => EntityModifyError::Sqlx(e),
                            })?;
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

    #[test]
    fn update_all_fn_no_columns() {
        let id = syn::parse_str("EntityId").unwrap();
        let entity = Ident::new("Entity", Span::call_site());

        let mut columns = Columns::default();
        columns.set_id_column(&id);

        let update_all_fn = UpdateAllFn {
            entity: &entity,
            table_name: "entities",
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
