use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::*;

/// `persist_events`: the shared single-entity events write.
///
/// SQLite: every write path (`create`, `update`, `delete`, `forget`) runs its
/// own index-row statement and then calls this, which inserts the events,
/// the forgettable payloads and (on a snapshot repo) upserts the snapshot
/// through `es_entity::sqlite_write`, all inside the caller's transaction.
pub struct PersistEventsFn<'a> {
    entity: &'a syn::Ident,
    id: &'a syn::Ident,
    event: &'a syn::Ident,
    events_table_name: &'a str,
    event_ctx: bool,
    forgettable_table_name: Option<&'a str>,
    snapshot_table_name: Option<&'a str>,
}

impl<'a> From<&'a RepositoryOptions> for PersistEventsFn<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            entity: opts.entity(),
            id: opts.id(),
            event: opts.event(),
            events_table_name: opts.events_table_name(),
            event_ctx: opts.event_context_enabled(),
            forgettable_table_name: opts.forgettable_table_name(),
            snapshot_table_name: opts.snapshot_table_name(),
        }
    }
}

impl ToTokens for PersistEventsFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let id_type = &self.id;
        let event_type = &self.event;
        let entity = &self.entity;
        let events_table = self.events_table_name;
        let event_ctx = self.event_ctx;

        let (ctx_var, ctx_arg) = if event_ctx {
            (
                quote! { let contexts = events.serialize_new_event_contexts(); },
                quote! { contexts.as_deref() },
            )
        } else {
            (quote! {}, quote! { None })
        };

        // A snapshot repo with no persisted index columns takes its snapshot
        // through this shared path: the caller computes `HeadSnapshot::capture()`
        // and passes the result in (`forget_in_op` passes `None`).
        // `persist_events` never compacts — the caller does that after its own
        // post-persist hook.
        let (events_ty, snapshot_param, snap_gather, snap_upsert) = match self.snapshot_table_name {
            Some(snap_tbl) => (
                quote! { es_entity::EntityEvents<#event_type, <#entity as es_entity::EsEntity>::Snapshot> },
                quote! { , snapshot: Option<&<#entity as es_entity::EsEntity>::Snapshot> },
                quote! {
                    let __snapshot_json = snapshot.map(|s| {
                        es_entity::prelude::serde_json::to_value(s).expect("Failed to serialize snapshot")
                    });
                    let __snapshot_head = (events.len_persisted() + events.len_new()) as i32;
                    let __snapshot_first = events.entity_first_persisted_at();
                },
                quote! {
                    if let Some(__json) = &__snapshot_json {
                        es_entity::sqlite_write::upsert_snapshot(
                            op.as_executor(),
                            #snap_tbl,
                            __now,
                            id,
                            __snapshot_head,
                            <<#entity as es_entity::EsEntity>::Snapshot as es_entity::EsSnapshot>::FINGERPRINT,
                            __json,
                            __snapshot_first,
                        )
                        .await?;
                    }
                },
            ),
            None => (
                quote! { es_entity::EntityEvents<#event_type> },
                quote! {},
                quote! {},
                quote! {},
            ),
        };

        let forgettable_code = if let Some(forgettable_tbl) = self.forgettable_table_name {
            // Upsert only when a snapshot is possible through this path: a
            // re-snapshot writes over the same `sequence = 0` row, while
            // event-only rows never conflict.
            let upsert = self.snapshot_table_name.is_some();
            let (snap_push, snap_delete) = if upsert {
                let delete = crate::runtime_query::query(
                    &format!("DELETE FROM {forgettable_tbl} WHERE entity_id = $1 AND sequence = 0"),
                    &[quote! { id as &#id_type }],
                );
                (
                    quote! {
                        if let Some(payload) = snapshot.and_then(es_entity::EsSnapshot::extract_forgettable_payloads) {
                            payload_sequences.push(0);
                            payload_values.push(payload);
                        }
                    },
                    quote! {
                        if snapshot.is_some()
                            && snapshot.and_then(es_entity::EsSnapshot::extract_forgettable_payloads).is_none()
                        {
                            #delete.execute(op.as_executor()).await?;
                        }
                    },
                )
            } else {
                (quote! {}, quote! {})
            };
            quote! {
                let mut payload_sequences: Vec<i32> = Vec::new();
                let mut payload_values: Vec<es_entity::prelude::serde_json::Value> = Vec::new();
                #snap_push
                for (idx, event_with_ctx) in events.iter_new_events().enumerate() {
                    if let Some(payload) = #event_type::extract_forgettable_payloads(&event_with_ctx.event) {
                        payload_sequences.push((offset + 1 + idx) as i32);
                        payload_values.push(payload);
                    }
                }
                if !payload_sequences.is_empty() {
                    let payload_ids = vec![id; payload_sequences.len()];
                    es_entity::sqlite_write::insert_payloads(
                        op.as_executor(),
                        #forgettable_tbl,
                        &payload_ids,
                        &payload_sequences,
                        &payload_values,
                        #upsert,
                    )
                    .await?;
                }
                #snap_delete
            }
        } else {
            quote! {}
        };

        tokens.append_all(quote! {
            async fn persist_events<OP>(
                &self,
                op: &mut OP,
                events: &mut #events_ty
                #snapshot_param
            ) -> Result<usize, sqlx::Error>
            where
                OP: es_entity::AtomicOperation + ?Sized,
            {
                let id = events.id();
                if !events.any_new() {
                    return Ok(0);
                }
                let offset = events.len_persisted();
                let events_types = events.new_event_types();
                let serialized_events = events.serialize_new_events();
                #ctx_var
                #snap_gather
                let __now = es_entity::sqlite_write::resolve_now(op.maybe_now());

                es_entity::sqlite_write::insert_entity_events(
                    op.as_executor(),
                    #events_table,
                    #event_ctx,
                    __now,
                    id,
                    offset,
                    &events_types,
                    &serialized_events,
                    #ctx_arg,
                )
                .await?;
                #forgettable_code
                #snap_upsert

                let n_events = events.mark_new_events_persisted_at(__now);

                Ok(n_events)
            }
        });
    }
}
