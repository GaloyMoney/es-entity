use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::*;

pub struct UpdateFn<'a> {
    entity: &'a syn::Ident,
    table_name: &'a str,
    columns: &'a Columns,
    modify_error: syn::Ident,
    nested_fn_names: Vec<syn::Ident>,
    #[cfg(feature = "instrument")]
    repo_name_snake: String,
}

impl<'a> From<&'a RepositoryOptions> for UpdateFn<'a> {
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

impl ToTokens for UpdateFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let entity = self.entity;
        let modify_error = &self.modify_error;

        let nested = self.nested_fn_names.iter().map(|f| {
            quote! {
                self.#f(op, entity).await?;
            }
        });

        let update_tokens = if self.columns.updates_needed() {
            let assignments = self
                .columns
                .variable_assignments_for_update(syn::parse_quote! { entity });
            let column_updates = self.columns.sql_updates();
            let query = format!(
                "UPDATE {} SET {} WHERE id = $1",
                self.table_name, column_updates,
            );
            let args = self.columns.update_query_args();
            Some(quote! {
            #assignments
            sqlx::query!(
                #query,
                #(#args),*
            )
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
            })
        } else {
            None
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

                    #update_tokens
                    let n_events = {
                        let events = Self::extract_events(entity);
                        self.persist_events::<_, #modify_error>(op, events).await?
                    };

                    self.execute_post_persist_hook(op, &entity, entity.events().last_persisted(n_events)).await.map_err(#modify_error::PostPersistHookError)?;

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

        let update_fn = UpdateFn {
            entity: &entity,
            table_name: "entities",
            modify_error: syn::Ident::new("EntityModifyError", Span::call_site()),
            columns: &columns,
            nested_fn_names: Vec::new(),
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
                    sqlx::query!(
                        "UPDATE entities SET name = $2 WHERE id = $1",
                        id as &EntityId,
                        name as &String
                    )
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

                    let n_events = {
                        let events = Self::extract_events(entity);
                        self.persist_events::<_, EntityModifyError>(op, events).await?
                    };

                    self.execute_post_persist_hook(op, &entity, entity.events().last_persisted(n_events)).await.map_err(EntityModifyError::PostPersistHookError)?;

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

        let update_fn = UpdateFn {
            entity: &entity,
            table_name: "entities",
            modify_error: syn::Ident::new("EntityModifyError", Span::call_site()),
            columns: &columns,
            nested_fn_names: Vec::new(),
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
                        self.persist_events::<_, EntityModifyError>(op, events).await?
                    };

                    self.execute_post_persist_hook(op, &entity, entity.events().last_persisted(n_events)).await.map_err(EntityModifyError::PostPersistHookError)?;

                    Ok(n_events)
                }.await;

                __result
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
