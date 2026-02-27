use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::*;

pub struct CreateFn<'a> {
    entity: &'a syn::Ident,
    table_name: &'a str,
    columns: &'a Columns,
    create_error: syn::Ident,
    nested_fn_names: Vec<syn::Ident>,
    #[cfg(feature = "instrument")]
    repo_name_snake: String,
}

impl<'a> From<&'a RepositoryOptions> for CreateFn<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            table_name: opts.table_name(),
            entity: opts.entity(),
            create_error: opts.create_error(),
            nested_fn_names: opts
                .all_nested()
                .map(|f| f.create_nested_fn_name())
                .collect(),
            columns: &opts.columns,
            #[cfg(feature = "instrument")]
            repo_name_snake: opts.repo_name_snake_case(),
        }
    }
}

impl ToTokens for CreateFn<'_> {
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
        let assignments = self
            .columns
            .variable_assignments_for_create(syn::parse_quote! { new_entity });

        let table_name = self.table_name;

        let column_names = self.columns.insert_column_names();
        let placeholders = self.columns.insert_placeholders(0);
        let args = self.columns.create_query_args();

        let query = format!(
            "INSERT INTO {} ({}, created_at) VALUES ({}, COALESCE(${}, NOW()))",
            table_name,
            column_names.join(", "),
            placeholders,
            column_names.len() + 1,
        );

        #[cfg(feature = "instrument")]
        let (instrument_attr, record_id, error_recording) = {
            let entity_name = entity.to_string();
            let repo_name = &self.repo_name_snake;
            let span_name = format!("{}.create", repo_name);
            (
                quote! {
                    #[tracing::instrument(name = #span_name, skip_all, fields(entity = #entity_name, id = tracing::field::Empty, error = tracing::field::Empty, exception.message = tracing::field::Empty, exception.type = tracing::field::Empty))]
                },
                quote! {
                    tracing::Span::current().record("id", tracing::field::debug(&id));
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
            fn convert_new<Entity, Event>(item: Entity) -> es_entity::EntityEvents<Event>
            where
                Entity: es_entity::IntoEvents<Event>,
                Event: es_entity::EsEvent,
            {
                item.into_events()
            }

            #[inline(always)]
            fn hydrate_entity<Entity, Event>(events: es_entity::EntityEvents<Event>) -> Result<Entity, es_entity::EntityHydrationError>
            where
                Entity: es_entity::TryFromEvents<Event>,
                Event: es_entity::EsEvent,
            {
                Entity::try_from_events(events)
            }

            pub async fn create(
                &self,
                new_entity: <#entity as es_entity::EsEntity>::New
            ) -> Result<#entity, #create_error> {
                let mut op = self.begin_op().await?;
                let res = self.create_in_op(&mut op, new_entity).await?;
                op.commit().await?;
                Ok(res)
            }

            #instrument_attr
            pub async fn create_in_op<OP>(
                &self,
                op: &mut OP,
                new_entity: <#entity as es_entity::EsEntity>::New
            ) -> Result<#entity, #create_error>
            where
                OP: es_entity::AtomicOperation
            {
                let __result: Result<#entity, #create_error> = async {
                    #assignments
                    #record_id

                     sqlx::query!(
                         #query,
                         #(#args)*
                         op.maybe_now()
                    )
                    .execute(op.as_executor())
                    .await
                    .map_err(|e| match &e {
                        sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
                            #create_error::ConstraintViolation {
                                column: Self::map_constraint_column(db_err.constraint()),
                                inner: e,
                            }
                        }
                        _ => #create_error::Sqlx(e),
                    })?;

                    let mut events = Self::convert_new(new_entity);
                    let n_events = self.persist_events::<_, #create_error>(op, &mut events).await?;
                    let #maybe_mut_entity = Self::hydrate_entity(events)?;

                    #(#nested)*

                    self.execute_post_persist_hook(op, &entity, entity.events().last_persisted(n_events)).await.map_err(#create_error::PostPersistHookError)?;
                    Ok(entity)
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
    fn create_fn() {
        let entity = Ident::new("Entity", Span::call_site());
        let create_error = syn::Ident::new("EntityCreateError", Span::call_site());
        let id = Ident::new("EntityId", Span::call_site());
        let mut columns = Columns::default();
        columns.set_id_column(&id);

        let create_fn = CreateFn {
            table_name: "entities",
            entity: &entity,
            create_error,
            columns: &columns,
            nested_fn_names: Vec::new(),
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        create_fn.to_tokens(&mut tokens);

        let expected = quote! {
            #[inline(always)]
            fn convert_new<Entity, Event>(item: Entity) -> es_entity::EntityEvents<Event>
            where
                Entity: es_entity::IntoEvents<Event>,
                Event: es_entity::EsEvent,
            {
                item.into_events()
            }

            #[inline(always)]
            fn hydrate_entity<Entity, Event>(events: es_entity::EntityEvents<Event>) -> Result<Entity, es_entity::EntityHydrationError>
            where
                Entity: es_entity::TryFromEvents<Event>,
                Event: es_entity::EsEvent,
            {
                Entity::try_from_events(events)
            }

            pub async fn create(
                &self,
                new_entity: <Entity as es_entity::EsEntity>::New
            ) -> Result<Entity, EntityCreateError> {
                let mut op = self.begin_op().await?;
                let res = self.create_in_op(&mut op, new_entity).await?;
                op.commit().await?;
                Ok(res)
            }

            pub async fn create_in_op<OP>(
                &self,
                op: &mut OP,
                new_entity: <Entity as es_entity::EsEntity>::New
            ) -> Result<Entity, EntityCreateError>
            where
                OP: es_entity::AtomicOperation
            {
                let __result: Result<Entity, EntityCreateError> = async {
                    let id = &new_entity.id;

                    sqlx::query!("INSERT INTO entities (id, created_at) VALUES ($1, COALESCE($2, NOW()))",
                        id as &EntityId,
                        op.maybe_now()
                    )
                    .execute(op.as_executor())
                    .await
                    .map_err(|e| match &e {
                        sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
                            EntityCreateError::ConstraintViolation {
                                column: Self::map_constraint_column(db_err.constraint()),
                                inner: e,
                            }
                        }
                        _ => EntityCreateError::Sqlx(e),
                    })?;

                    let mut events = Self::convert_new(new_entity);
                    let n_events = self.persist_events::<_, EntityCreateError>(op, &mut events).await?;
                    let entity = Self::hydrate_entity(events)?;

                    self.execute_post_persist_hook(op, &entity, entity.events().last_persisted(n_events)).await.map_err(EntityCreateError::PostPersistHookError)?;
                    Ok(entity)
                }.await;

                __result
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn create_fn_with_columns() {
        let entity = Ident::new("Entity", Span::call_site());
        let create_error = syn::Ident::new("EntityCreateError", Span::call_site());

        use darling::FromMeta;
        let input: syn::Meta = syn::parse_quote!(columns(
            id = "EntityId",
            name(ty = "String", create(accessor = "name()"))
        ));
        let columns = Columns::from_meta(&input).expect("Failed to parse Fields");

        let create_fn = CreateFn {
            table_name: "entities",
            entity: &entity,
            create_error,
            columns: &columns,
            nested_fn_names: Vec::new(),
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        create_fn.to_tokens(&mut tokens);

        let expected = quote! {
            #[inline(always)]
            fn convert_new<Entity, Event>(item: Entity) -> es_entity::EntityEvents<Event>
            where
                Entity: es_entity::IntoEvents<Event>,
                Event: es_entity::EsEvent,
            {
                item.into_events()
            }

            #[inline(always)]
            fn hydrate_entity<Entity, Event>(events: es_entity::EntityEvents<Event>) -> Result<Entity, es_entity::EntityHydrationError>
            where
                Entity: es_entity::TryFromEvents<Event>,
                Event: es_entity::EsEvent,
            {
                Entity::try_from_events(events)
            }

            pub async fn create(
                &self,
                new_entity: <Entity as es_entity::EsEntity>::New
            ) -> Result<Entity, EntityCreateError> {
                let mut op = self.begin_op().await?;
                let res = self.create_in_op(&mut op, new_entity).await?;
                op.commit().await?;
                Ok(res)
            }

            pub async fn create_in_op<OP>(
                &self,
                op: &mut OP,
                new_entity: <Entity as es_entity::EsEntity>::New
            ) -> Result<Entity, EntityCreateError>
            where
                OP: es_entity::AtomicOperation
            {
                let __result: Result<Entity, EntityCreateError> = async {
                    let id = &new_entity.id;
                    let name = &new_entity.name();

                    sqlx::query!("INSERT INTO entities (id, name, created_at) VALUES ($1, $2, COALESCE($3, NOW()))",
                        id as &EntityId,
                        name as &String,
                        op.maybe_now()
                    )
                    .execute(op.as_executor())
                    .await
                    .map_err(|e| match &e {
                        sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
                            EntityCreateError::ConstraintViolation {
                                column: Self::map_constraint_column(db_err.constraint()),
                                inner: e,
                            }
                        }
                        _ => EntityCreateError::Sqlx(e),
                    })?;

                    let mut events = Self::convert_new(new_entity);
                    let n_events = self.persist_events::<_, EntityCreateError>(op, &mut events).await?;
                    let entity = Self::hydrate_entity(events)?;

                    self.execute_post_persist_hook(op, &entity, entity.events().last_persisted(n_events)).await.map_err(EntityCreateError::PostPersistHookError)?;
                    Ok(entity)
                }.await;

                __result
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
