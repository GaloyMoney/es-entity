use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::*;

/// `persist_events_batch`: the shared many-entity events write (SQLite:
/// multi-row inserts through `es_entity::sqlite_write`). Snapshots are not
/// written here; callers upsert them themselves.
pub struct PersistEventsBatchFn<'a> {
    entity: &'a syn::Ident,
    id: &'a syn::Ident,
    event: &'a syn::Ident,
    events_table_name: &'a str,
    event_ctx: bool,
    forgettable_table_name: Option<&'a str>,
    snapshot_enabled: bool,
}

impl<'a> From<&'a RepositoryOptions> for PersistEventsBatchFn<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            entity: opts.entity(),
            id: opts.id(),
            event: opts.event(),
            events_table_name: opts.events_table_name(),
            event_ctx: opts.event_context_enabled(),
            forgettable_table_name: opts.forgettable_table_name(),
            snapshot_enabled: opts.snapshot_enabled(),
        }
    }
}

impl ToTokens for PersistEventsBatchFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let id_type = &self.id;
        let event_type = &self.event;
        let entity = &self.entity;
        let events_table = self.events_table_name;
        let event_ctx = self.event_ctx;
        let events_ty = if self.snapshot_enabled {
            quote! { es_entity::EntityEvents<#event_type, <#entity as es_entity::EsEntity>::Snapshot> }
        } else {
            quote! { es_entity::EntityEvents<#event_type> }
        };

        let (ctx_var, ctx_extend, ctx_arg) = if event_ctx {
            (
                quote! {
                    let mut all_contexts: Vec<es_entity::ContextData> = Vec::new();
                },
                quote! {
                    if let Some(contexts) = events.serialize_new_event_contexts() {
                        all_contexts.extend(contexts);
                    }
                },
                quote! {
                    if all_contexts.is_empty() { None } else { Some(all_contexts.as_slice()) }
                },
            )
        } else {
            (quote! {}, quote! {}, quote! { None })
        };

        let (forgettable_vars, forgettable_extract, forgettable_insert) = match self
            .forgettable_table_name
        {
            Some(forgettable_tbl) => (
                quote! {
                    let mut payload_ids: Vec<&#id_type> = Vec::new();
                    let mut payload_sequences: Vec<i32> = Vec::new();
                    let mut payload_values: Vec<es_entity::prelude::serde_json::Value> = Vec::new();
                },
                quote! {
                    for (idx, event_with_ctx) in events.iter_new_events().enumerate() {
                        if let Some(payload) = #event_type::extract_forgettable_payloads(&event_with_ctx.event) {
                            payload_ids.push(id);
                            payload_sequences.push((offset + idx) as i32);
                            payload_values.push(payload);
                        }
                    }
                },
                quote! {
                    if !payload_sequences.is_empty() {
                        es_entity::sqlite_write::insert_payloads(
                            op.as_executor(),
                            #forgettable_tbl,
                            &payload_ids,
                            &payload_sequences,
                            &payload_values,
                            false,
                        )
                        .await?;
                    }
                },
            ),
            None => (quote! {}, quote! {}, quote! {}),
        };

        tokens.append_all(quote! {
            async fn persist_events_batch<OP, B>(
                &self,
                op: &mut OP,
                all_events: &mut [B]
            ) -> Result<std::collections::HashMap<#id_type, usize>, sqlx::Error>
            where
                OP: es_entity::AtomicOperation + ?Sized,
                B: std::borrow::BorrowMut<#events_ty>,
            {
                let mut all_serialized = Vec::new();
                #ctx_var
                #forgettable_vars
                let mut all_types = Vec::new();
                let mut all_ids: Vec<&#id_type> = Vec::new();
                let mut all_sequences = Vec::new();
                let __now = es_entity::sqlite_write::resolve_now(op.maybe_now());

                let mut n_events_map = std::collections::HashMap::new();
                for item in all_events.iter() {
                    let events: &#events_ty = item.borrow();
                    let id = events.id();
                    let offset = events.len_persisted() + 1;
                    let types = events.new_event_types();
                    let serialized = events.serialize_new_events();
                    #ctx_extend
                    #forgettable_extract

                    let n_events = serialized.len();
                    all_serialized.extend(serialized);
                    all_types.extend(types);
                    all_ids.extend(std::iter::repeat(id).take(n_events));
                    all_sequences.extend((offset..).take(n_events).map(|i| i as i32));
                    n_events_map.insert(id.clone(), n_events);
                }

                if all_ids.is_empty() {
                    return Ok(n_events_map);
                }

                es_entity::sqlite_write::insert_events(
                    op.as_executor(),
                    #events_table,
                    #event_ctx,
                    __now,
                    &all_ids,
                    &all_sequences,
                    &all_types,
                    &all_serialized,
                    #ctx_arg,
                )
                .await?;

                #forgettable_insert

                for item in all_events.iter_mut() {
                    let events: &mut #events_ty = item.borrow_mut();
                    events.mark_new_events_persisted_at(__now);
                }

                Ok(n_events_map)
            }
        });
    }
}
