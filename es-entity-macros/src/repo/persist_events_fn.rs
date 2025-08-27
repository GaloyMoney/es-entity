use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::*;

pub struct PersistEventsFn<'a> {
    id: &'a syn::Ident,
    event: &'a syn::Ident,
    error: &'a syn::Type,
    events_table_name: &'a str,
    event_ctx: bool,
}

impl<'a> From<&'a RepositoryOptions> for PersistEventsFn<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            id: opts.id(),
            event: opts.event(),
            error: opts.err(),
            events_table_name: opts.events_table_name(),
            event_ctx: opts.event_ctx,
        }
    }
}

impl ToTokens for PersistEventsFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let context = self.event_ctx;
        let query = format!(
            "INSERT INTO {} (id, recorded_at, sequence, event_type, event{}) SELECT $1, COALESCE($2, NOW()), ROW_NUMBER() OVER () + $3, unnested.event_type, unnested.event{} FROM UNNEST($4::text[], $5::jsonb[]) AS unnested(event_type, event) RETURNING recorded_at",
            self.events_table_name,
            if context { ", context" } else { "" },
            if context { ", $6" } else { "" }
        );
        let (context, ctx_arg) = if context {
            (
                quote! { Some(es_entity::EventContext::current().as_json().expect("Couldn't serialize context")) },
                quote! { Self::current_context() },
            )
        } else {
            (quote! { None }, quote! {})
        };

        let id_type = &self.id;
        let event_type = &self.event;
        let error = self.error;
        let id_tokens = quote! {
            id as &#id_type
        };

        tokens.append_all(quote! {
            fn current_context() -> Option<serde_json::Value> {
                #context
            }

            fn extract_concurrent_modification<T>(res: Result<T, sqlx::Error>) -> Result<T, #error> {
                match res {
                    Ok(entity) => Ok(entity),
                    Err(sqlx::Error::Database(db_error)) if db_error.is_unique_violation() => {
                        Err(#error::from(es_entity::EsEntityError::ConcurrentModification))
                    }
                    Err(err) => Err(#error::from(err)),
                }
            }

            async fn persist_events<OP>(
                &self,
                op: &mut OP,
                events: &mut es_entity::EntityEvents<#event_type>
            ) -> Result<usize, #error>
            where
                OP: es_entity::AtomicOperation
            {
                let id = events.id();
                let offset = events.len_persisted();
                let serialized_events = events.serialize_new_events();
                let events_types = serialized_events.iter().map(|e| e.get("type").and_then(es_entity::prelude::serde_json::Value::as_str).expect("Could not read event type").to_owned()).collect::<Vec<_>>();
                let now = op.now();

                let rows = Self::extract_concurrent_modification(
                    sqlx::query!(
                        #query,
                        #id_tokens,
                        now,
                        offset as i32,
                        &events_types,
                        &serialized_events,
                        #ctx_arg
                    ).fetch_all(op.as_executor()).await)?;

                let recorded_at = rows[0].recorded_at;
                let n_events = events.mark_new_events_persisted_at(recorded_at);

                Ok(n_events)
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persist_events_fn() {
        let id = syn::parse_str("EntityId").unwrap();
        let event = syn::Ident::new("EntityEvent", proc_macro2::Span::call_site());
        let error = syn::parse_str("es_entity::EsRepoError").unwrap();
        let persist_fn = PersistEventsFn {
            id: &id,
            event: &event,
            error: &error,
            events_table_name: "entity_events",
            event_ctx: false,
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
            fn current_context() -> Option<serde_json::Value> {
                None
            }

            fn extract_concurrent_modification<T>(res: Result<T, sqlx::Error>) -> Result<T, es_entity::EsRepoError> {
                match res {
                    Ok(entity) => Ok(entity),
                    Err(sqlx::Error::Database(db_error)) if db_error.is_unique_violation() => {
                        Err(es_entity::EsRepoError::from(es_entity::EsEntityError::ConcurrentModification))
                    }
                    Err(err) => Err(es_entity::EsRepoError::from(err)),
                }
            }

            async fn persist_events<OP>(
                &self,
                op: &mut OP,
                events: &mut es_entity::EntityEvents<EntityEvent>
            ) -> Result<usize, es_entity::EsRepoError>
            where
                OP: es_entity::AtomicOperation
            {
                let id = events.id();
                let offset = events.len_persisted();
                let serialized_events = events.serialize_new_events();
                let events_types = serialized_events.iter().map(|e| e.get("type").and_then(es_entity::prelude::serde_json::Value::as_str).expect("Could not read event type").to_owned()).collect::<Vec<_>>();
                let now = op.now();

                let rows = Self::extract_concurrent_modification(
                    sqlx::query!(
                        "INSERT INTO entity_events (id, recorded_at, sequence, event_type, event) SELECT $1, COALESCE($2, NOW()), ROW_NUMBER() OVER () + $3, unnested.event_type, unnested.event FROM UNNEST($4::text[], $5::jsonb[]) AS unnested(event_type, event) RETURNING recorded_at",
                        id as &EntityId,
                        now,
                        offset as i32,
                        &events_types,
                        &serialized_events,
                    ).fetch_all(op.as_executor()).await)?;

                let recorded_at = rows[0].recorded_at;
                let n_events = events.mark_new_events_persisted_at(recorded_at);

                Ok(n_events)
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
