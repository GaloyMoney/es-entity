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
                        .await
                        .map_err(|e| match &e {
                            sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
                                #modify_error::ConstraintViolation {
                                    column: Self::map_constraint_column(db_err.constraint()),
                                    inner: e,
                                }
                            }
                            _ => #modify_error::Sqlx(e),
                        })?;
                }),
            )
        } else {
            (None, None, None)
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

                    #vec_declarations

                    let mut has_new_events = false;
                    for entity in entities.iter() {
                        if !entity.events().any_new() {
                            continue;
                        }
                        has_new_events = true;

                        #per_entity_pushes
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
                    let n_persisted = self.persist_events_batch::<_, _, #modify_error>(op, &mut all_event_refs).await?;
                    drop(all_event_refs);

                    let mut total_events = 0usize;
                    for entity in entities.iter_mut() {
                        if let Some(&n_events) = n_persisted.get(&entity.id) {
                            if n_events > 0 {
                                self.execute_post_persist_hook(op, &entity, entity.events().last_persisted(n_events)).await.map_err(#modify_error::PostPersistHookError)?;
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

                    let mut id_collection = Vec::new();
                    let mut name_collection = Vec::new();

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
                    }

                    if !has_new_events {
                        return Ok(0);
                    }

                    sqlx::query("UPDATE entities SET name = unnested.name FROM UNNEST($1, $2) AS unnested(id, name) WHERE entities.id = unnested.id")
                        .bind(id_collection)
                        .bind(name_collection)
                        .execute(op.as_executor())
                        .await
                        .map_err(|e| match &e {
                            sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
                                EntityModifyError::ConstraintViolation {
                                    column: Self::map_constraint_column(db_err.constraint()),
                                    inner: e,
                                }
                            }
                            _ => EntityModifyError::Sqlx(e),
                        })?;

                    let mut all_event_refs: Vec<_> = entities.iter_mut()
                        .filter_map(|entity| {
                            let events = Self::extract_events(entity);
                            if events.any_new() { Some(events) } else { None }
                        })
                        .collect();
                    let n_persisted = self.persist_events_batch::<_, _, EntityModifyError>(op, &mut all_event_refs).await?;
                    drop(all_event_refs);

                    let mut total_events = 0usize;
                    for entity in entities.iter_mut() {
                        if let Some(&n_events) = n_persisted.get(&entity.id) {
                            if n_events > 0 {
                                self.execute_post_persist_hook(op, &entity, entity.events().last_persisted(n_events)).await.map_err(EntityModifyError::PostPersistHookError)?;
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
                    let n_persisted = self.persist_events_batch::<_, _, EntityModifyError>(op, &mut all_event_refs).await?;
                    drop(all_event_refs);

                    let mut total_events = 0usize;
                    for entity in entities.iter_mut() {
                        if let Some(&n_events) = n_persisted.get(&entity.id) {
                            if n_events > 0 {
                                self.execute_post_persist_hook(op, &entity, entity.events().last_persisted(n_events)).await.map_err(EntityModifyError::PostPersistHookError)?;
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
