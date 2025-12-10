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

        let query = format!(
            "INSERT INTO {} (id, recorded_at, sequence, event_type, event{}) \
             SELECT unnested.id, COALESCE($1, NOW()), unnested.sequence, unnested.event_type, unnested.event{} \
             FROM UNNEST($2, $3::INT[], $4::TEXT[], $5::JSONB[]{}) \
             AS unnested(id, sequence, event_type, event{}) RETURNING recorded_at",
            self.events_table_name,
            if self.event_ctx { ", context" } else { "" },
            if self.event_ctx {
                ", unnested.context"
            } else {
                ""
            },
            if self.event_ctx { ", $6::JSONB[]" } else { "" },
            if self.event_ctx { ", context" } else { "" }
        );

        let (ctx_var, ctx_extend, ctx_bind) = if self.event_ctx {
            (
                quote! {
                    let mut all_contexts: Vec<es_entity::ContextData> = Vec::new();
                },
                quote! {
                    let contexts = events.serialize_new_event_contexts();
                    if let Some(contexts) = contexts {
                        all_contexts.extend(contexts);
                    }
                },
                quote! {
                    .bind(&if all_contexts.is_empty() {
                        None
                    } else {
                         Some(all_contexts)
                    })
                },
            )
        } else {
            (quote! {}, quote! {}, quote! {})
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
                use es_entity::prelude::sqlx::Row;

                let mut all_serialized = Vec::new();
                #ctx_var
                let mut all_types = Vec::new();
                let mut all_ids: Vec<&#id_type> = Vec::new();
                let mut all_sequences = Vec::new();
                let now = op.maybe_now();

                let mut n_events_map = std::collections::HashMap::new();
                for events in all_events.iter_mut() {
                    let id = events.id();
                    let offset = events.len_persisted() + 1;
                    let serialized = events.serialize_new_events();
                    #ctx_extend
                    let types = serialized.iter()
                        .map(|e| e.get("type")
                            .and_then(es_entity::prelude::serde_json::Value::as_str)
                            .expect("Could not read event type")
                            .to_owned())
                        .collect::<Vec<_>>();

                    let n_events = serialized.len();
                    all_serialized.extend(serialized);
                    all_types.extend(types);
                    all_ids.extend(std::iter::repeat(id).take(n_events));
                    all_sequences.extend((offset..).take(n_events).map(|i| i as i32));
                    n_events_map.insert(id.clone(), n_events);
                }

                let rows = Self::extract_concurrent_modification(
                    sqlx::query(#query)
                        .bind(now)
                        .bind(&all_ids)
                        .bind(&all_sequences)
                        .bind(&all_types)
                        .bind(&all_serialized)
                        #ctx_bind
                        .fetch_all(op.as_executor())
                        .await
                )?;

                let recorded_at = rows[0].try_get("recorded_at").expect("no recorded at");

                for events in all_events.iter_mut() {
                    events.mark_new_events_persisted_at(recorded_at);
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
                use es_entity::prelude::sqlx::Row;

                let mut all_serialized = Vec::new();
                let mut all_contexts: Vec<es_entity::ContextData> = Vec::new();
                let mut all_types = Vec::new();
                let mut all_ids: Vec<&EntityId> = Vec::new();
                let mut all_sequences = Vec::new();
                let now = op.maybe_now();

                let mut n_events_map = std::collections::HashMap::new();
                for events in all_events.iter_mut() {
                    let id = events.id();
                    let offset = events.len_persisted() + 1;
                    let serialized = events.serialize_new_events();
                    let contexts = events.serialize_new_event_contexts();
                    if let Some(contexts) = contexts {
                        all_contexts.extend(contexts);
                    }
                    let types = serialized.iter()
                        .map(|e| e.get("type")
                            .and_then(es_entity::prelude::serde_json::Value::as_str)
                            .expect("Could not read event type")
                            .to_owned())
                        .collect::<Vec<_>>();

                    let n_events = serialized.len();
                    all_serialized.extend(serialized);
                    all_types.extend(types);
                    all_ids.extend(std::iter::repeat(id).take(n_events));
                    all_sequences.extend((offset..).take(n_events).map(|i| i as i32));
                    n_events_map.insert(id.clone(), n_events);
                }

                let rows = Self::extract_concurrent_modification(
                    sqlx::query("INSERT INTO entity_events (id, recorded_at, sequence, event_type, event, context) SELECT unnested.id, COALESCE($1, NOW()), unnested.sequence, unnested.event_type, unnested.event, unnested.context FROM UNNEST($2, $3::INT[], $4::TEXT[], $5::JSONB[], $6::JSONB[]) AS unnested(id, sequence, event_type, event, context) RETURNING recorded_at")
                        .bind(now)
                        .bind(&all_ids)
                        .bind(&all_sequences)
                        .bind(&all_types)
                        .bind(&all_serialized)
                        .bind(&if all_contexts.is_empty() {
                            None
                        } else {
                             Some(all_contexts)
                        })
                        .fetch_all(op.as_executor())
                        .await
                )?;

                let recorded_at = rows[0].try_get("recorded_at").expect("no recorded at");

                for events in all_events.iter_mut() {
                    events.mark_new_events_persisted_at(recorded_at);
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
                use es_entity::prelude::sqlx::Row;

                let mut all_serialized = Vec::new();
                let mut all_types = Vec::new();
                let mut all_ids: Vec<&EntityId> = Vec::new();
                let mut all_sequences = Vec::new();
                let now = op.maybe_now();

                let mut n_events_map = std::collections::HashMap::new();
                for events in all_events.iter_mut() {
                    let id = events.id();
                    let offset = events.len_persisted() + 1;
                    let serialized = events.serialize_new_events();
                    let types = serialized.iter()
                        .map(|e| e.get("type")
                            .and_then(es_entity::prelude::serde_json::Value::as_str)
                            .expect("Could not read event type")
                            .to_owned())
                        .collect::<Vec<_>>();

                    let n_events = serialized.len();
                    all_serialized.extend(serialized);
                    all_types.extend(types);
                    all_ids.extend(std::iter::repeat(id).take(n_events));
                    all_sequences.extend((offset..).take(n_events).map(|i| i as i32));
                    n_events_map.insert(id.clone(), n_events);
                }

                let rows = Self::extract_concurrent_modification(
                    sqlx::query("INSERT INTO entity_events (id, recorded_at, sequence, event_type, event) SELECT unnested.id, COALESCE($1, NOW()), unnested.sequence, unnested.event_type, unnested.event FROM UNNEST($2, $3::INT[], $4::TEXT[], $5::JSONB[]) AS unnested(id, sequence, event_type, event) RETURNING recorded_at")
                        .bind(now)
                        .bind(&all_ids)
                        .bind(&all_sequences)
                        .bind(&all_types)
                        .bind(&all_serialized)
                        .fetch_all(op.as_executor())
                        .await
                )?;

                let recorded_at = rows[0].try_get("recorded_at").expect("no recorded at");

                for events in all_events.iter_mut() {
                    events.mark_new_events_persisted_at(recorded_at);
                }

                Ok(n_events_map)
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
