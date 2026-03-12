use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::*;

pub struct PersistEventsBatchFn<'a> {
    id: &'a syn::Ident,
    event: &'a syn::Ident,
    events_table_name: &'a str,
    event_ctx: bool,
}

impl<'a> From<&'a RepositoryOptions> for PersistEventsBatchFn<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            id: opts.id(),
            event: opts.event(),
            events_table_name: opts.events_table_name(),
            event_ctx: opts.event_context_enabled(),
        }
    }
}

impl ToTokens for PersistEventsBatchFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let id_type = &self.id;
        let event_type = &self.event;
        let events_table_name = self.events_table_name;

        let (insert_query, ctx_var, ctx_extend, ctx_bind) = if self.event_ctx {
            (
                format!(
                    "INSERT INTO {} (id, recorded_at, sequence, event_type, event, context) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    events_table_name
                ),
                quote! {
                    let mut all_contexts: Vec<Option<es_entity::ContextData>> = Vec::new();
                },
                quote! {
                    let contexts = events.serialize_new_event_contexts();
                    if let Some(contexts) = contexts {
                        all_contexts.extend(contexts.into_iter().map(Some));
                    } else {
                        all_contexts.extend(std::iter::repeat(None).take(n_events));
                    }
                },
                quote! {
                    let context = all_contexts.get(i).and_then(|c| c.as_ref());
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
                quote! {},
            )
        };

        tokens.append_all(quote! {
            async fn persist_events_batch<OP, B>(
                &self,
                op: &mut OP,
                all_events: &mut [B]
            ) -> Result<std::collections::HashMap<#id_type, usize>, sqlx::Error>
            where
                OP: es_entity::AtomicOperation,
                B: std::borrow::BorrowMut<es_entity::EntityEvents<#event_type>>,
            {
                let mut all_serialized = Vec::new();
                #ctx_var
                let mut all_types = Vec::new();
                let mut all_ids: Vec<&#id_type> = Vec::new();
                let mut all_sequences = Vec::new();
                let now = op.maybe_now();
                let recorded_at = now.unwrap_or_else(|| es_entity::prelude::chrono::Utc::now());

                let mut n_events_map = std::collections::HashMap::new();
                for item in all_events.iter() {
                    let events: &es_entity::EntityEvents<#event_type> = item.borrow();
                    let id = events.id();
                    let offset = events.len_persisted() + 1;
                    let types = events.new_event_types();
                    let serialized = events.serialize_new_events();

                    let n_events = serialized.len();
                    #ctx_extend
                    all_serialized.extend(serialized);
                    all_types.extend(types);
                    all_ids.extend(std::iter::repeat(id).take(n_events));
                    all_sequences.extend((offset..).take(n_events).map(|i| i as i64));
                    n_events_map.insert(id.clone(), n_events);
                }

                for (i, ((id, sequence), (event_type, event_json))) in all_ids.iter()
                    .zip(all_sequences.iter())
                    .zip(all_types.iter().zip(all_serialized.iter()))
                    .enumerate()
                {
                    let mut query = sqlx::query(#insert_query)
                        .bind(*id as &#id_type)
                        .bind(recorded_at)
                        .bind(*sequence)
                        .bind(event_type)
                        .bind(event_json);
                    #ctx_bind
                    query.execute(op.as_executor()).await?;
                }

                for item in all_events.iter_mut() {
                    let events: &mut es_entity::EntityEvents<#event_type> = item.borrow_mut();
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
        let persist_fn = PersistEventsBatchFn {
            id: &id,
            event: &event,
            events_table_name: "entity_events",
            event_ctx: true,
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
            async fn persist_events_batch<OP, B>(
                &self,
                op: &mut OP,
                all_events: &mut [B]
            ) -> Result<std::collections::HashMap<EntityId, usize>, sqlx::Error>
            where
                OP: es_entity::AtomicOperation,
                B: std::borrow::BorrowMut<es_entity::EntityEvents<EntityEvent>>,
            {
                let mut all_serialized = Vec::new();
                let mut all_contexts: Vec<Option<es_entity::ContextData>> = Vec::new();
                let mut all_types = Vec::new();
                let mut all_ids: Vec<&EntityId> = Vec::new();
                let mut all_sequences = Vec::new();
                let now = op.maybe_now();
                let recorded_at = now.unwrap_or_else(|| es_entity::prelude::chrono::Utc::now());

                let mut n_events_map = std::collections::HashMap::new();
                for item in all_events.iter() {
                    let events: &es_entity::EntityEvents<EntityEvent> = item.borrow();
                    let id = events.id();
                    let offset = events.len_persisted() + 1;
                    let types = events.new_event_types();
                    let serialized = events.serialize_new_events();

                    let n_events = serialized.len();
                    let contexts = events.serialize_new_event_contexts();
                    if let Some(contexts) = contexts {
                        all_contexts.extend(contexts.into_iter().map(Some));
                    } else {
                        all_contexts.extend(std::iter::repeat(None).take(n_events));
                    }
                    all_serialized.extend(serialized);
                    all_types.extend(types);
                    all_ids.extend(std::iter::repeat(id).take(n_events));
                    all_sequences.extend((offset..).take(n_events).map(|i| i as i64));
                    n_events_map.insert(id.clone(), n_events);
                }

                for (i, ((id, sequence), (event_type, event_json))) in all_ids.iter()
                    .zip(all_sequences.iter())
                    .zip(all_types.iter().zip(all_serialized.iter()))
                    .enumerate()
                {
                    let mut query = sqlx::query("INSERT INTO entity_events (id, recorded_at, sequence, event_type, event, context) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")
                        .bind(*id as &EntityId)
                        .bind(recorded_at)
                        .bind(*sequence)
                        .bind(event_type)
                        .bind(event_json);
                    let context = all_contexts.get(i).and_then(|c| c.as_ref());
                    query = query.bind(context);
                    query.execute(op.as_executor()).await?;
                }

                for item in all_events.iter_mut() {
                    let events: &mut es_entity::EntityEvents<EntityEvent> = item.borrow_mut();
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
        let persist_fn = PersistEventsBatchFn {
            id: &id,
            event: &event,
            events_table_name: "entity_events",
            event_ctx: false,
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
            async fn persist_events_batch<OP, B>(
                &self,
                op: &mut OP,
                all_events: &mut [B]
            ) -> Result<std::collections::HashMap<EntityId, usize>, sqlx::Error>
            where
                OP: es_entity::AtomicOperation,
                B: std::borrow::BorrowMut<es_entity::EntityEvents<EntityEvent>>,
            {
                let mut all_serialized = Vec::new();
                let mut all_types = Vec::new();
                let mut all_ids: Vec<&EntityId> = Vec::new();
                let mut all_sequences = Vec::new();
                let now = op.maybe_now();
                let recorded_at = now.unwrap_or_else(|| es_entity::prelude::chrono::Utc::now());

                let mut n_events_map = std::collections::HashMap::new();
                for item in all_events.iter() {
                    let events: &es_entity::EntityEvents<EntityEvent> = item.borrow();
                    let id = events.id();
                    let offset = events.len_persisted() + 1;
                    let types = events.new_event_types();
                    let serialized = events.serialize_new_events();

                    let n_events = serialized.len();
                    all_serialized.extend(serialized);
                    all_types.extend(types);
                    all_ids.extend(std::iter::repeat(id).take(n_events));
                    all_sequences.extend((offset..).take(n_events).map(|i| i as i64));
                    n_events_map.insert(id.clone(), n_events);
                }

                for (i, ((id, sequence), (event_type, event_json))) in all_ids.iter()
                    .zip(all_sequences.iter())
                    .zip(all_types.iter().zip(all_serialized.iter()))
                    .enumerate()
                {
                    let mut query = sqlx::query("INSERT INTO entity_events (id, recorded_at, sequence, event_type, event) VALUES (?1, ?2, ?3, ?4, ?5)")
                        .bind(*id as &EntityId)
                        .bind(recorded_at)
                        .bind(*sequence)
                        .bind(event_type)
                        .bind(event_json);
                    query.execute(op.as_executor()).await?;
                }

                for item in all_events.iter_mut() {
                    let events: &mut es_entity::EntityEvents<EntityEvent> = item.borrow_mut();
                    events.mark_new_events_persisted_at(recorded_at);
                }

                Ok(n_events_map)
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
