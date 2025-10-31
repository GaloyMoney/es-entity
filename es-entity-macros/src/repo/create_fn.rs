use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::*;

pub struct CreateFn<'a> {
    entity: &'a syn::Ident,
    table_name: &'a str,
    columns: &'a Columns,
    error: &'a syn::Type,
    nested_fn_names: Vec<syn::Ident>,
    additional_op_constraint: proc_macro2::TokenStream,
    #[cfg(feature = "instrument")]
    repo_name_snake: String,
}

impl<'a> From<&'a RepositoryOptions> for CreateFn<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            table_name: opts.table_name(),
            entity: opts.entity(),
            error: opts.err(),
            nested_fn_names: opts
                .all_nested()
                .map(|f| f.create_nested_fn_name())
                .collect(),
            columns: &opts.columns,
            additional_op_constraint: opts.additional_op_constraint(),
            #[cfg(feature = "instrument")]
            repo_name_snake: opts.repo_name_snake_case(),
        }
    }
}

impl ToTokens for CreateFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let entity = self.entity;
        let error = self.error;
        let additional_op_constraint = &self.additional_op_constraint;

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
                    #[tracing::instrument(name = #span_name, skip_all, fields(entity = #entity_name, id = tracing::field::Empty, exception.message = tracing::field::Empty, exception.type = tracing::field::Empty))]
                },
                quote! {
                    tracing::Span::current().record("id", tracing::field::debug(&id));
                },
                quote! {
                    if let Err(ref e) = __result {
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
            fn hydrate_entity<Entity, Event>(events: es_entity::EntityEvents<Event>) -> Result<Entity, #error>
            where
                Entity: es_entity::TryFromEvents<Event>,
                #error: From<es_entity::EsEntityError>,
                Event: es_entity::EsEvent,
            {
                Ok(Entity::try_from_events(events)?)
            }

            pub async fn create(
                &self,
                new_entity: <#entity as es_entity::EsEntity>::New
            ) -> Result<#entity, #error> {
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
            ) -> Result<#entity, #error>
            where
                OP: es_entity::AtomicOperation
                #additional_op_constraint
            {
                let __result: Result<#entity, #error> = async {
                    #assignments
                    #record_id

                     sqlx::query!(
                         #query,
                         #(#args)*
                         op.now()
                    )
                    .execute(op.as_executor())
                    .await?;

                    let mut events = Self::convert_new(new_entity);
                    let n_events = self.persist_events(op, &mut events).await?;
                    let #maybe_mut_entity = Self::hydrate_entity(events)?;

                    #(#nested)*

                    self.execute_post_persist_hook(op, &entity, entity.events().last_persisted(n_events)).await?;
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
        let error = syn::parse_str("es_entity::EsRepoError").unwrap();
        let id = Ident::new("EntityId", Span::call_site());
        let mut columns = Columns::default();
        columns.set_id_column(&id);

        let create_fn = CreateFn {
            table_name: "entities",
            entity: &entity,
            error: &error,
            columns: &columns,
            nested_fn_names: Vec::new(),
            additional_op_constraint: quote! {},
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
            fn hydrate_entity<Entity, Event>(events: es_entity::EntityEvents<Event>) -> Result<Entity, es_entity::EsRepoError>
            where
                Entity: es_entity::TryFromEvents<Event>,
                es_entity::EsRepoError: From<es_entity::EsEntityError>,
                Event: es_entity::EsEvent,
            {
                Ok(Entity::try_from_events(events)?)
            }

            pub async fn create(
                &self,
                new_entity: <Entity as es_entity::EsEntity>::New
            ) -> Result<Entity, es_entity::EsRepoError> {
                let mut op = self.begin_op().await?;
                let res = self.create_in_op(&mut op, new_entity).await?;
                op.commit().await?;
                Ok(res)
            }

            pub async fn create_in_op<OP>(
                &self,
                op: &mut OP,
                new_entity: <Entity as es_entity::EsEntity>::New
            ) -> Result<Entity, es_entity::EsRepoError>
            where
                OP: es_entity::AtomicOperation
            {
                let __result: Result<Entity, es_entity::EsRepoError> = async {
                    let id = &new_entity.id;

                    sqlx::query!("INSERT INTO entities (id, created_at) VALUES ($1, COALESCE($2, NOW()))",
                        id as &EntityId,
                        op.now()
                    )
                    .execute(op.as_executor())
                    .await?;

                    let mut events = Self::convert_new(new_entity);
                    let n_events = self.persist_events(op, &mut events).await?;
                    let entity = Self::hydrate_entity(events)?;

                    self.execute_post_persist_hook(op, &entity, entity.events().last_persisted(n_events)).await?;
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
        let error = syn::parse_str("es_entity::EsRepoError").unwrap();

        use darling::FromMeta;
        let input: syn::Meta = syn::parse_quote!(columns(
            id = "EntityId",
            name(ty = "String", create(accessor = "name()"))
        ));
        let columns = Columns::from_meta(&input).expect("Failed to parse Fields");

        let create_fn = CreateFn {
            table_name: "entities",
            entity: &entity,
            error: &error,
            columns: &columns,
            nested_fn_names: Vec::new(),
            additional_op_constraint: quote! {},
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
            fn hydrate_entity<Entity, Event>(events: es_entity::EntityEvents<Event>) -> Result<Entity, es_entity::EsRepoError>
            where
                Entity: es_entity::TryFromEvents<Event>,
                es_entity::EsRepoError: From<es_entity::EsEntityError>,
                Event: es_entity::EsEvent,
            {
                Ok(Entity::try_from_events(events)?)
            }

            pub async fn create(
                &self,
                new_entity: <Entity as es_entity::EsEntity>::New
            ) -> Result<Entity, es_entity::EsRepoError> {
                let mut op = self.begin_op().await?;
                let res = self.create_in_op(&mut op, new_entity).await?;
                op.commit().await?;
                Ok(res)
            }

            pub async fn create_in_op<OP>(
                &self,
                op: &mut OP,
                new_entity: <Entity as es_entity::EsEntity>::New
            ) -> Result<Entity, es_entity::EsRepoError>
            where
                OP: es_entity::AtomicOperation
            {
                let __result: Result<Entity, es_entity::EsRepoError> = async {
                    let id = &new_entity.id;
                    let name = &new_entity.name();

                    sqlx::query!("INSERT INTO entities (id, name, created_at) VALUES ($1, $2, COALESCE($3, NOW()))",
                        id as &EntityId,
                        name as &String,
                        op.now()
                    )
                    .execute(op.as_executor())
                    .await?;

                    let mut events = Self::convert_new(new_entity);
                    let n_events = self.persist_events(op, &mut events).await?;
                    let entity = Self::hydrate_entity(events)?;

                    self.execute_post_persist_hook(op, &entity, entity.events().last_persisted(n_events)).await?;
                    Ok(entity)
                }.await;

                __result
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
