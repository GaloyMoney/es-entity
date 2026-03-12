use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::*;

pub struct PersistEventsFn<'a> {
    id: &'a syn::Ident,
    event: &'a syn::Ident,
    events_table_name: &'a str,
    event_ctx: bool,
}

impl<'a> From<&'a RepositoryOptions> for PersistEventsFn<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            id: opts.id(),
            event: opts.event(),
            events_table_name: opts.events_table_name(),
            event_ctx: opts.event_context_enabled(),
        }
    }
}

impl ToTokens for PersistEventsFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let events_table_name = self.events_table_name;

        let (insert_query, ctx_var, ctx_bind) = if self.event_ctx {
            (
                format!(
                    "INSERT INTO {} (id, recorded_at, sequence, event_type, event, context) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    events_table_name
                ),
                quote! { let contexts = events.serialize_new_event_contexts(); },
                quote! {
                    let context = contexts.as_ref().and_then(|c| c.get(i));
                    query = query.bind(context);
                },
            )
        } else {
            (
                format!(
                    "INSERT INTO {} (id, recorded_at, sequence, event_type, event) VALUES (?1, ?2, ?3, ?4, ?5)",
                    events_table_name
                ),
                quote! {},
                quote! {},
            )
        };
        let id_type = &self.id;
        let event_type = &self.event;

        tokens.append_all(quote! {
            fn extract_concurrent_modification<T, __EsErr: From<sqlx::Error>>(
                res: Result<T, sqlx::Error>,
                concurrent_modification: __EsErr,
            ) -> Result<T, __EsErr> {
                match res {
                    Ok(v) => Ok(v),
                    Err(sqlx::Error::Database(ref db_err)) if db_err.is_unique_violation() => {
                        Err(concurrent_modification)
                    }
                    Err(e) => Err(__EsErr::from(e)),
                }
            }

            async fn persist_events<OP>(
                &self,
                op: &mut OP,
                events: &mut es_entity::EntityEvents<#event_type>
            ) -> Result<usize, sqlx::Error>
            where
                OP: es_entity::AtomicOperation,
            {
                let id = events.id();
                let offset = events.len_persisted();
                let events_types = events.new_event_types();
                let serialized_events = events.serialize_new_events();
                #ctx_var
                let now = op.maybe_now();
                let recorded_at = now.unwrap_or_else(|| es_entity::prelude::chrono::Utc::now());

                for (i, (event_type, event_json)) in events_types.iter().zip(serialized_events.iter()).enumerate() {
                    let sequence = offset as i64 + i as i64 + 1;
                    let mut query = sqlx::query(#insert_query)
                        .bind(id as &#id_type)
                        .bind(recorded_at)
                        .bind(sequence)
                        .bind(event_type)
                        .bind(event_json);
                    #ctx_bind
                    query.execute(op.as_executor()).await?;
                }

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
        let persist_fn = PersistEventsFn {
            id: &id,
            event: &event,
            events_table_name: "entity_events",
            event_ctx: true,
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
            fn extract_concurrent_modification<T, __EsErr: From<sqlx::Error>>(
                res: Result<T, sqlx::Error>,
                concurrent_modification: __EsErr,
            ) -> Result<T, __EsErr> {
                match res {
                    Ok(v) => Ok(v),
                    Err(sqlx::Error::Database(ref db_err)) if db_err.is_unique_violation() => {
                        Err(concurrent_modification)
                    }
                    Err(e) => Err(__EsErr::from(e)),
                }
            }

            async fn persist_events<OP>(
                &self,
                op: &mut OP,
                events: &mut es_entity::EntityEvents<EntityEvent>
            ) -> Result<usize, sqlx::Error>
            where
                OP: es_entity::AtomicOperation,
            {
                let id = events.id();
                let offset = events.len_persisted();
                let events_types = events.new_event_types();
                let serialized_events = events.serialize_new_events();
                let contexts = events.serialize_new_event_contexts();
                let now = op.maybe_now();
                let recorded_at = now.unwrap_or_else(|| es_entity::prelude::chrono::Utc::now());

                for (i, (event_type, event_json)) in events_types.iter().zip(serialized_events.iter()).enumerate() {
                    let sequence = offset as i64 + i as i64 + 1;
                    let mut query = sqlx::query("INSERT INTO entity_events (id, recorded_at, sequence, event_type, event, context) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")
                        .bind(id as &EntityId)
                        .bind(recorded_at)
                        .bind(sequence)
                        .bind(event_type)
                        .bind(event_json);
                    let context = contexts.as_ref().and_then(|c| c.get(i));
                    query = query.bind(context);
                    query.execute(op.as_executor()).await?;
                }

                let n_events = events.mark_new_events_persisted_at(recorded_at);

                Ok(n_events)
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn persist_events_fn_without_event_context() {
        let id = syn::parse_str("EntityId").unwrap();
        let event = syn::Ident::new("EntityEvent", proc_macro2::Span::call_site());
        let persist_fn = PersistEventsFn {
            id: &id,
            event: &event,
            events_table_name: "entity_events",
            event_ctx: false,
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
            fn extract_concurrent_modification<T, __EsErr: From<sqlx::Error>>(
                res: Result<T, sqlx::Error>,
                concurrent_modification: __EsErr,
            ) -> Result<T, __EsErr> {
                match res {
                    Ok(v) => Ok(v),
                    Err(sqlx::Error::Database(ref db_err)) if db_err.is_unique_violation() => {
                        Err(concurrent_modification)
                    }
                    Err(e) => Err(__EsErr::from(e)),
                }
            }

            async fn persist_events<OP>(
                &self,
                op: &mut OP,
                events: &mut es_entity::EntityEvents<EntityEvent>
            ) -> Result<usize, sqlx::Error>
            where
                OP: es_entity::AtomicOperation,
            {
                let id = events.id();
                let offset = events.len_persisted();
                let events_types = events.new_event_types();
                let serialized_events = events.serialize_new_events();
                let now = op.maybe_now();
                let recorded_at = now.unwrap_or_else(|| es_entity::prelude::chrono::Utc::now());

                for (i, (event_type, event_json)) in events_types.iter().zip(serialized_events.iter()).enumerate() {
                    let sequence = offset as i64 + i as i64 + 1;
                    let mut query = sqlx::query("INSERT INTO entity_events (id, recorded_at, sequence, event_type, event) VALUES (?1, ?2, ?3, ?4, ?5)")
                        .bind(id as &EntityId)
                        .bind(recorded_at)
                        .bind(sequence)
                        .bind(event_type)
                        .bind(event_json);
                    query.execute(op.as_executor()).await?;
                }

                let n_events = events.mark_new_events_persisted_at(recorded_at);

                Ok(n_events)
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
