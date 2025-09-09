use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::*;

pub struct PersistEventsBatchFn<'a> {
    id: &'a syn::Ident,
    event: &'a syn::Ident,
    error: &'a syn::Type,
    events_table_name: &'a str,
    event_ctx: bool,
}

impl<'a> From<&'a RepositoryOptions> for PersistEventsBatchFn<'a> {
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

impl ToTokens for PersistEventsBatchFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let id_type = &self.id;
        let event_type = &self.event;
        let error = self.error;

        let copy_query = format!(
            "COPY {} (id, sequence, event_type, event{}) FROM STDIN WITH (FORMAT text)",
            self.events_table_name,
            if self.event_ctx { ", context" } else { "" }
        );

        let copy_data_generation = if self.event_ctx {
            quote! {
                for events in all_events.iter_mut() {
                    let id = events.id();
                    let offset = events.len_persisted();
                    let serialized = events.serialize_new_events();
                    let contexts = events.serialize_new_event_contexts();
                    
                    for (idx, (event, context)) in serialized.into_iter().zip(contexts.unwrap_or_default().into_iter()).enumerate() {
                        let event_type = event
                            .get("type")
                            .and_then(es_entity::prelude::serde_json::Value::as_str)
                            .expect("Could not read event type");
                        let row = format!(
                            "{}\t{}\t{}\t{}\t{}\n",
                            id,
                            offset + idx + 1,
                            event_type,
                            es_entity::prelude::serde_json::to_string(&event).expect("event to string")
                                .replace("\\", "\\\\"),
                            match context {
                                Some(ctx) => es_entity::prelude::serde_json::to_string(&ctx).expect("context to string"),
                                None => "\\N".to_string(),
                            }
                        );
                        copy.send(row.as_bytes()).await?;
                    }
                }
            }
        } else {
            quote! {
                for events in all_events.iter_mut() {
                    let id = events.id();
                    let offset = events.len_persisted();
                    let serialized = events.serialize_new_events();
                    
                    for (idx, event) in serialized.into_iter().enumerate() {
                        let event_type = event
                            .get("type")
                            .and_then(es_entity::prelude::serde_json::Value::as_str)
                            .expect("Could not read event type");
                        let row = format!(
                            "{}\t{}\t{}\t{}\n",
                            id,
                            offset + idx + 1,
                            event_type,
                            es_entity::prelude::serde_json::to_string(&event).expect("event to string")
                                .replace("\\", "\\\\")
                        );
                        copy.send(row.as_bytes()).await?;
                    }
                }
            }
        };

        tokens.append_all(quote! {
            async fn persist_events_batch<OP>(
                &self,
                op: &mut OP,
                all_events: &mut [es_entity::EntityEvents<#event_type>]
            ) -> Result<std::collections::HashMap<#id_type, usize>, #error>
            where
                OP: es_entity::AtomicOperation
            {
                // Check if there are any events to persist
                let total_events: usize = all_events.iter().map(|e| e.serialize_new_events().len()).sum();
                if total_events == 0 {
                    return Ok(std::collections::HashMap::new());
                }

                let mut n_events_map = std::collections::HashMap::new();
                
                // Collect event counts for return value
                for events in all_events.iter() {
                    let id = events.id();
                    let n_events = events.serialize_new_events().len();
                    if n_events > 0 {
                        n_events_map.insert(id.clone(), n_events);
                    }
                }

                // Perform the COPY operation
                let _rows_copied = Self::extract_concurrent_modification({
                    let mut copy = op
                        .as_executor()
                        .copy_in_raw(#copy_query)
                        .await?;

                    #copy_data_generation

                    copy.finish().await
                })?;

                // Mark all events as persisted with current timestamp
                let recorded_at = es_entity::prelude::chrono::Utc::now();
                for events in all_events.iter_mut() {
                    if !events.serialize_new_events().is_empty() {
                        events.mark_new_events_persisted_at(recorded_at);
                    }
                }

                Ok(n_events_map)
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
        let persist_fn = PersistEventsBatchFn {
            id: &id,
            event: &event,
            error: &error,
            events_table_name: "entity_events",
            event_ctx: true,
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
            async fn persist_events_batch<OP>(
                &self,
                op: &mut OP,
                all_events: &mut [es_entity::EntityEvents<EntityEvent>]
            ) -> Result<std::collections::HashMap<EntityId, usize>, es_entity::EsRepoError>
            where
                OP: es_entity::AtomicOperation
            {
                // Check if there are any events to persist
                let total_events: usize = all_events.iter().map(|e| e.serialize_new_events().len()).sum();
                if total_events == 0 {
                    return Ok(std::collections::HashMap::new());
                }

                let mut n_events_map = std::collections::HashMap::new();
                
                // Collect event counts for return value
                for events in all_events.iter() {
                    let id = events.id();
                    let n_events = events.serialize_new_events().len();
                    if n_events > 0 {
                        n_events_map.insert(id.clone(), n_events);
                    }
                }

                // Perform the COPY operation
                let _rows_copied = Self::extract_concurrent_modification({
                    let mut copy = op
                        .as_executor()
                        .copy_in_raw("COPY entity_events (id, sequence, event_type, event, context) FROM STDIN WITH (FORMAT text)")
                        .await?;

                    for events in all_events.iter_mut() {
                        let id = events.id();
                        let offset = events.len_persisted();
                        let serialized = events.serialize_new_events();
                        let contexts = events.serialize_new_event_contexts();
                        
                        for (idx, (event, context)) in serialized.into_iter().zip(contexts.unwrap_or_default().into_iter()).enumerate() {
                            let event_type = event
                                .get("type")
                                .and_then(es_entity::prelude::serde_json::Value::as_str)
                                .expect("Could not read event type");
                            let row = format!(
                                "{}\t{}\t{}\t{}\t{}\n",
                                id,
                                offset + idx + 1,
                                event_type,
                                es_entity::prelude::serde_json::to_string(&event).expect("event to string")
                                    .replace("\\", "\\\\"),
                                match context {
                                    Some(ctx) => es_entity::prelude::serde_json::to_string(&ctx).expect("context to string"),
                                    None => "\\N".to_string(),
                                }
                            );
                            copy.send(row.as_bytes()).await?;
                        }
                    }

                    copy.finish().await
                })?;

                // Mark all events as persisted with current timestamp
                let recorded_at = es_entity::prelude::chrono::Utc::now();
                for events in all_events.iter_mut() {
                    if !events.serialize_new_events().is_empty() {
                        events.mark_new_events_persisted_at(recorded_at);
                    }
                }

                Ok(n_events_map)
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn persist_events_fn_without_event_context() {
        let id = syn::parse_str("EntityId").unwrap();
        let event = syn::Ident::new("EntityEvent", proc_macro2::Span::call_site());
        let error = syn::parse_str("es_entity::EsRepoError").unwrap();
        let persist_fn = PersistEventsBatchFn {
            id: &id,
            event: &event,
            error: &error,
            events_table_name: "entity_events",
            event_ctx: false,
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
            async fn persist_events_batch<OP>(
                &self,
                op: &mut OP,
                all_events: &mut [es_entity::EntityEvents<EntityEvent>]
            ) -> Result<std::collections::HashMap<EntityId, usize>, es_entity::EsRepoError>
            where
                OP: es_entity::AtomicOperation
            {
                // Check if there are any events to persist
                let total_events: usize = all_events.iter().map(|e| e.serialize_new_events().len()).sum();
                if total_events == 0 {
                    return Ok(std::collections::HashMap::new());
                }

                let mut n_events_map = std::collections::HashMap::new();
                
                // Collect event counts for return value
                for events in all_events.iter() {
                    let id = events.id();
                    let n_events = events.serialize_new_events().len();
                    if n_events > 0 {
                        n_events_map.insert(id.clone(), n_events);
                    }
                }

                // Perform the COPY operation
                let _rows_copied = Self::extract_concurrent_modification({
                    let mut copy = op
                        .as_executor()
                        .copy_in_raw("COPY entity_events (id, sequence, event_type, event) FROM STDIN WITH (FORMAT text)")
                        .await?;

                    for events in all_events.iter_mut() {
                        let id = events.id();
                        let offset = events.len_persisted();
                        let serialized = events.serialize_new_events();
                        
                        for (idx, event) in serialized.into_iter().enumerate() {
                            let event_type = event
                                .get("type")
                                .and_then(es_entity::prelude::serde_json::Value::as_str)
                                .expect("Could not read event type");
                            let row = format!(
                                "{}\t{}\t{}\t{}\n",
                                id,
                                offset + idx + 1,
                                event_type,
                                es_entity::prelude::serde_json::to_string(&event).expect("event to string")
                                    .replace("\\", "\\\\")
                            );
                            copy.send(row.as_bytes()).await?;
                        }
                    }

                    copy.finish().await
                })?;

                // Mark all events as persisted with current timestamp
                let recorded_at = es_entity::prelude::chrono::Utc::now();
                for events in all_events.iter_mut() {
                    if !events.serialize_new_events().is_empty() {
                        events.mark_new_events_persisted_at(recorded_at);
                    }
                }

                Ok(n_events_map)
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
