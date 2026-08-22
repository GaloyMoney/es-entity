use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::{
    events_write::{EventSource, EventsInsert, ForgettablePayloads},
    options::*,
};

pub struct CreateFn<'a> {
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

impl<'a> From<&'a RepositoryOptions> for CreateFn<'a> {
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

impl ToTokens for CreateFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let entity = self.entity;
        let create_error = &self.create_error;

        let nested = self.nested_fn_names.iter().map(|f| {
            quote! {
                self.#f(op, &mut [&mut entity]).await?;
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
        let arg_adds = self.columns.create_query_arg_adds();

        // Index insert and events insert combined into one statement. The
        // outer insert reads from the CTE, which provides the id without a
        // second bind. Postgres interleaves the CTE and main inserts with no
        // guaranteed ordering, so a duplicate id may surface as either
        // table's constraint — the classifier maps both to the same
        // `ConstraintViolation`.
        //
        // A brand-new entity has no persisted events, so sequences start at 1
        // and no offset parameter is needed.
        let events_insert = EventsInsert::new(self.events_table_name, self.event_ctx);
        let now_p = column_names.len() + 1;
        let source = EventSource::PerEntityCte {
            cte: "new_row",
            offset_param: None,
        };
        let query = format!(
            "WITH new_row AS (INSERT INTO {} ({}, created_at) VALUES ({}, COALESCE(${}, NOW())) RETURNING id) {}",
            table_name,
            column_names.join(", "),
            placeholders,
            now_p,
            events_insert.sql(&source, now_p, now_p + 1),
        );

        // The event arrays are encoded after the index columns, whose borrows
        // on the new entity must end before it is consumed into events.
        let event_arg_adds = events_insert
            .arg_exprs(&source)
            .into_iter()
            .skip(1)
            .map(|expr| quote! { __query_args.add(#expr).map_err(sqlx::Error::Encode)?; });
        let ctx_var = if self.event_ctx {
            quote! { let contexts = events.serialize_new_event_contexts(); }
        } else {
            quote! {}
        };

        let forgettable_code = match self.forgettable_table_name {
            Some(table) => {
                let payloads = ForgettablePayloads {
                    table,
                    id_type: self.id,
                    event_type: self.event,
                }
                .insert_per_entity(quote! { events }, create_error);
                quote! {
                    let offset = events.len_persisted();
                    let id = events.id();
                    #payloads
                }
            }
            None => quote! {},
        };

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
                    use es_entity::prelude::sqlx::{Arguments, Row};

                    #assignments
                    #record_id

                    let mut __query_args = sqlx::postgres::PgArguments::default();
                    #(#arg_adds)*
                    __query_args.add(op.maybe_now()).map_err(sqlx::Error::Encode)?;

                    let mut events = Self::convert_new(new_entity);
                    let events_types = events.new_event_types();
                    let serialized_events = events.serialize_new_events();
                    #ctx_var
                    #(#event_arg_adds)*

                    let rows = sqlx::query_with(#query, __query_args)
                        .fetch_all(op.as_executor())
                        .await
                        .map_err(Self::classify_create_error)?;

                    #forgettable_code

                    let recorded_at = rows
                        .first()
                        .ok_or(sqlx::Error::RowNotFound)
                        .and_then(|row| row.try_get("recorded_at"))?;
                    let n_events = events.mark_new_events_persisted_at(recorded_at);
                    let #maybe_mut_entity = Self::hydrate_entity(events)?;

                    #(#nested)*

                    #post_hydrate_check
                    #post_persist_check
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
        let event = Ident::new("EntityEvent", Span::call_site());
        let mut columns = Columns::default();
        columns.set_id_column(&id);

        let create_fn = CreateFn {
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
                    use es_entity::prelude::sqlx::{Arguments, Row};

                    let id = &new_entity.id;

                    let mut __query_args = sqlx::postgres::PgArguments::default();
                    __query_args.add(id as &EntityId).map_err(sqlx::Error::Encode)?;
                    __query_args.add(op.maybe_now()).map_err(sqlx::Error::Encode)?;

                    let mut events = Self::convert_new(new_entity);
                    let events_types = events.new_event_types();
                    let serialized_events = events.serialize_new_events();
                    __query_args.add(&events_types).map_err(sqlx::Error::Encode)?;
                    __query_args.add(&serialized_events).map_err(sqlx::Error::Encode)?;

                    let rows = sqlx::query_with(
                        "WITH new_row AS (INSERT INTO entities (id, created_at) VALUES ($1, COALESCE($2, NOW())) RETURNING id) INSERT INTO entity_events (id, recorded_at, sequence, event_type, event) SELECT new_row.id, COALESCE($2, NOW()), ROW_NUMBER() OVER (), unnested.event_type, unnested.event FROM new_row CROSS JOIN UNNEST($3::TEXT[], $4::JSONB[]) AS unnested(event_type, event) RETURNING recorded_at",
                        __query_args
                    )
                        .fetch_all(op.as_executor())
                        .await
                        .map_err(Self::classify_create_error)?;

                    let recorded_at = rows
                        .first()
                        .ok_or(sqlx::Error::RowNotFound)
                        .and_then(|row| row.try_get("recorded_at"))?;
                    let n_events = events.mark_new_events_persisted_at(recorded_at);
                    let entity = Self::hydrate_entity(events)?;

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
        let id = Ident::new("EntityId", Span::call_site());
        let event = Ident::new("EntityEvent", Span::call_site());

        use darling::FromMeta;
        let input: syn::Meta =
            syn::parse_quote!(columns(name(ty = "String", create(accessor = "name()"))));
        let mut columns = Columns::from_meta(&input).expect("Failed to parse Fields");
        columns.set_id_column(&id);

        let create_fn = CreateFn {
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
                    use es_entity::prelude::sqlx::{Arguments, Row};

                    let id = &new_entity.id;
                    let name = &new_entity.name();

                    let mut __query_args = sqlx::postgres::PgArguments::default();
                    __query_args.add(id as &EntityId).map_err(sqlx::Error::Encode)?;
                    __query_args.add(name as &String).map_err(sqlx::Error::Encode)?;
                    __query_args.add(op.maybe_now()).map_err(sqlx::Error::Encode)?;

                    let mut events = Self::convert_new(new_entity);
                    let events_types = events.new_event_types();
                    let serialized_events = events.serialize_new_events();
                    __query_args.add(&events_types).map_err(sqlx::Error::Encode)?;
                    __query_args.add(&serialized_events).map_err(sqlx::Error::Encode)?;

                    let rows = sqlx::query_with(
                        "WITH new_row AS (INSERT INTO entities (id, name, created_at) VALUES ($1, $2, COALESCE($3, NOW())) RETURNING id) INSERT INTO entity_events (id, recorded_at, sequence, event_type, event) SELECT new_row.id, COALESCE($3, NOW()), ROW_NUMBER() OVER (), unnested.event_type, unnested.event FROM new_row CROSS JOIN UNNEST($4::TEXT[], $5::JSONB[]) AS unnested(event_type, event) RETURNING recorded_at",
                        __query_args
                    )
                        .fetch_all(op.as_executor())
                        .await
                        .map_err(Self::classify_create_error)?;

                    let recorded_at = rows
                        .first()
                        .ok_or(sqlx::Error::RowNotFound)
                        .and_then(|row| row.try_get("recorded_at"))?;
                    let n_events = events.mark_new_events_persisted_at(recorded_at);
                    let entity = Self::hydrate_entity(events)?;

                    Ok(entity)
                }.await;

                __result
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
