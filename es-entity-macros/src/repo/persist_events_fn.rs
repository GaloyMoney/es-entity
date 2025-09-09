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
            event_ctx: opts.event_context_enabled(),
        }
    }
}

impl ToTokens for PersistEventsFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let copy_query = format!(
            "COPY {} (id, sequence, event_type, event{}) FROM STDIN WITH (FORMAT text)",
            self.events_table_name,
            if self.event_ctx { ", context" } else { "" }
        );

        let event_type = &self.event;
        let error = self.error;

        let (for_stmt, format_str, ctx_arg) = if self.event_ctx {
            (
                quote! { for (idx, (event, context)) in serialized_events.into_iter().zip(contexts.into_iter()).enumerate() },
                quote! {"{}\t{}\t{}\t{}\t{}\n"},
                quote! {
                match context {
                    Some(ctx) => es_entity::prelude::serde_json::to_string(&ctx).expect("context to string"),
                    None => "\\N".to_string(),
                },
                },
            )
        } else {
            (
                quote! { for (idx, event) in serialized_events.into_iter().enumerate() },
                quote! {"{}\t{}\t{}\t{}\n"},
                quote! {},
            )
        };

        tokens.append_all(quote! {
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
                if events.serialize_new_events().is_empty() {
                    return Ok(0);
                }

                let id = events.id();
                let offset = events.len_persisted();
                let serialized_events = events.serialize_new_events();

                let rows_copied = Self::extract_concurrent_modification({
                    let mut copy = op
                        .as_executor()
                        .copy_in_raw(#copy_query)
                        .await?;

                    #for_stmt {
                    let event_type = event
                        .get("type")
                        .and_then(es_entity::prelude::serde_json::Value::as_str)
                        .expect("Could not read event type");
                    let row = format!(
                        #format_str,
                        id,
                        offset + idx + 1,
                        event_type,
                        es_entity::prelude::serde_json::to_string(&event).expect("event to string")
                                  .replace("\\", "\\\\")
                        ,
                        #ctx_arg
                    );
                    copy.send(row.as_bytes()).await?;
                    }

                    copy.finish().await
                })?;

                // Mark events as persisted with current timestamp
                let recorded_at = es_entity::prelude::chrono::Utc::now();
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
            event_ctx: true,
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
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
                if events.serialize_new_events().is_empty() {
                    return Ok(0);
                }

                let id = events.id();
                let offset = events.len_persisted();
                let serialized_events = events.serialize_new_events();

                // Perform the COPY operation
                let rows_copied = Self::extract_concurrent_modification({
                    let mut copy = op
                        .as_executor()
                        .copy_in_raw("COPY entity_events (id, sequence, event_type, event, context, recorded_at) FROM STDIN WITH (FORMAT text)")
                        .await?;

                    let contexts = events.serialize_new_event_contexts();
                    for (idx, (event, context)) in serialized_events.into_iter().zip(contexts.unwrap_or_default().into_iter()).enumerate() {
                        let event_type = event
                            .get("type")
                            .and_then(es_entity::prelude::serde_json::Value::as_str)
                            .expect("Could not read event type");
                        let row = format!(
                            "{}\t{}\t{}\t{}\t{}\t{}\n",
                            id,
                            offset + idx,
                            event_type,
                            es_entity::prelude::serde_json::to_string(&event).expect("event to string"),
                            es_entity::prelude::serde_json::to_string(&context).expect("context to string"),
                            es_entity::prelude::chrono::Utc::now(),
                        );
                        copy.send(row.as_bytes()).await?;
                    }

                    copy.finish().await
                })?;

                // Mark events as persisted with current timestamp
                let recorded_at = es_entity::prelude::chrono::Utc::now();
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
                if events.serialize_new_events().is_empty() {
                    return Ok(0);
                }

                let id = events.id();
                let offset = events.len_persisted();
                let serialized_events = events.serialize_new_events();

                // Perform the COPY operation
                let rows_copied = Self::extract_concurrent_modification({
                    let mut copy = op
                        .as_executor()
                        .copy_in_raw("COPY entity_events (id, sequence, event_type, event, recorded_at) FROM STDIN WITH (FORMAT text)")
                        .await?;

                    for (idx, event) in serialized_events.into_iter().enumerate() {
                        let event_type = event
                            .get("type")
                            .and_then(es_entity::prelude::serde_json::Value::as_str)
                            .expect("Could not read event type")
                            .to_owned();
                        let row = format!(
                            "{}\t{}\t{}\t{}\t{}\n",
                            id,
                            offset + idx,
                            event_type,
                            es_entity::prelude::serde_json::to_string(&event).expect("event to string"),
                            es_entity::prelude::chrono::Utc::now(),
                        );
                        copy.send(row.as_bytes()).await?;
                    }

                    copy.finish().await
                })?;

                // Mark events as persisted with current timestamp
                let recorded_at = es_entity::prelude::chrono::Utc::now();
                let n_events = events.mark_new_events_persisted_at(recorded_at);

                Ok(n_events)
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
