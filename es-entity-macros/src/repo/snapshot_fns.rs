use proc_macro2::TokenStream;
use quote::quote;

use super::options::*;

/// Emitted only for `#[es_repo(snapshot)]` repos: two private helpers used by
/// `forget_in_op` and by the batch write paths' stale-snapshot refresh —
/// `__full_history_find_by_id_in_op` (loads an entity by replaying its full
/// event history, bypassing any stored snapshot) and
/// `__persist_snapshot_in_op` (writes `capture()`'s result for an entity with
/// no staged events, guarded so a concurrent writer wins).
pub struct SnapshotFns<'a> {
    entity: &'a syn::Ident,
    id: &'a syn::Ident,
    table_name: &'a str,
    events_table_name: &'a str,
    snapshot_table_name: &'a str,
    forgettable_table_name: Option<&'a str>,
    column_enum: syn::Ident,
    find_error: syn::Ident,
    modify_error: syn::Ident,
}

impl<'a> SnapshotFns<'a> {
    pub fn from(opts: &'a RepositoryOptions) -> Option<Self> {
        let snapshot_table_name = opts.snapshot_table_name()?;
        Some(Self {
            entity: opts.entity(),
            id: opts.id(),
            table_name: opts.table_name(),
            events_table_name: opts.events_table_name(),
            snapshot_table_name,
            forgettable_table_name: opts.forgettable_table_name(),
            column_enum: opts.column_enum(),
            find_error: opts.find_error(),
            modify_error: opts.modify_error(),
        })
    }

    /// Goes inside `impl #repo { .. }`.
    pub fn in_impl_tokens(&self) -> TokenStream {
        let entity = self.entity;
        let id_type = self.id;
        let modify_error = &self.modify_error;
        let find_error = &self.find_error;
        let column_enum = &self.column_enum;
        let snapshot_tbl = self.snapshot_table_name;
        let table_name = self.table_name;
        let events_table_name = self.events_table_name;
        let entity_name = entity.to_string();

        let forgettable_tbl_arg = match self.forgettable_table_name {
            Some(tbl) => quote! { forgettable_tbl = #tbl, },
            None => quote! {},
        };
        let find_by_id_sql = format!("SELECT id FROM {table_name} WHERE id = $1");
        // Bypasses the public `es_query!` wrapper (whose arms don't carry a
        // `snapshot_fingerprint` override) and calls the underlying proc
        // macro directly — this is the one place that needs the fingerprint
        // forced to `NO_SNAPSHOT_FINGERPRINT` rather than the type's own.
        let query_call = quote! {
            es_entity::expand_es_query!(
                entity = #entity,
                #forgettable_tbl_arg
                snapshot_tbl = #snapshot_tbl,
                snapshot_fingerprint = es_entity::NO_SNAPSHOT_FINGERPRINT,
                sql = #find_by_id_sql,
                args = [id as &#id_type,]
            )
        };

        // A stale row's own sequence can equal the head it is being
        // refreshed to (its shape changed, but nothing new was appended
        // since) — `>=` lets that refresh through; same-sequence,
        // same-fingerprint rewrites are idempotent.
        let persist_snapshot_query = format!(
            "INSERT INTO {snapshot_tbl} (id, sequence, fingerprint, snapshot, first_recorded_at, recorded_at) \
             SELECT $1, $2, $3, $4, COALESCE($5, COALESCE($6, NOW())), COALESCE($6, NOW()) \
             WHERE (SELECT MAX(sequence) FROM {events_table_name} WHERE id = $1) = $2 \
             ON CONFLICT (id) DO UPDATE SET sequence = EXCLUDED.sequence, fingerprint = EXCLUDED.fingerprint, \
             snapshot = EXCLUDED.snapshot, first_recorded_at = EXCLUDED.first_recorded_at, recorded_at = EXCLUDED.recorded_at \
             WHERE EXCLUDED.sequence >= {snapshot_tbl}.sequence \
             RETURNING recorded_at"
        );

        let (payload_upsert_sql, payload_delete_sql) = match self.forgettable_table_name {
            Some(tbl) => (
                format!(
                    "INSERT INTO {tbl} (entity_id, sequence, payload) VALUES ($1, 0, $2) \
                     ON CONFLICT (entity_id, sequence) DO UPDATE SET payload = EXCLUDED.payload"
                ),
                format!("DELETE FROM {tbl} WHERE entity_id = $1 AND sequence = 0"),
            ),
            None => (String::new(), String::new()),
        };
        let payload_code = if self.forgettable_table_name.is_some() {
            quote! {
                match <<#entity as es_entity::EsEntity>::Snapshot as es_entity::EsSnapshot>::extract_forgettable_payloads(&state) {
                    Some(payload) => {
                        sqlx::query!(#payload_upsert_sql, id as &#id_type, payload)
                            .execute(op.as_executor())
                            .await?;
                    }
                    None => {
                        sqlx::query!(#payload_delete_sql, id as &#id_type)
                            .execute(op.as_executor())
                            .await?;
                    }
                }
            }
        } else {
            quote! {}
        };

        quote! {
            async fn __full_history_find_by_id_in_op<'a, OP>(
                &self,
                op: OP,
                id: &#id_type,
            ) -> Result<#entity, #find_error>
            where
                OP: es_entity::IntoOneTimeExecutor<'a>,
            {
                #query_call
                    .fetch_optional(op)
                    .await?
                    .ok_or_else(|| #find_error::NotFound {
                        entity: #entity_name,
                        column: Some(#column_enum::Id),
                        value: {
                            use es_entity::ToNotFoundValueFallback;
                            es_entity::NotFoundValue(id).to_not_found_value()
                        },
                    })
            }

            /// Writes `capture()`'s result for an entity with no staged
            /// events, guarded so a concurrent writer (including a `forget`
            /// that staged its own erasure event) makes this a no-op rather
            /// than an error. Errors if `entity` has unpersisted staged
            /// events — persist them first.
            async fn __persist_snapshot_in_op<OP>(
                &self,
                op: &mut OP,
                entity: &mut #entity,
            ) -> Result<bool, #modify_error>
            where
                OP: es_entity::AtomicOperation + ?Sized,
            {
                if entity.events().any_new() {
                    return Err(#modify_error::ConcurrentModification);
                }
                let state = match <#entity as es_entity::HeadSnapshot>::capture(&*entity) {
                    Some(state) => state,
                    None => return Ok(false),
                };
                let snapshot_json = es_entity::prelude::serde_json::to_value(&state)
                    .expect("Failed to serialize snapshot");
                let head = entity.events().len_persisted() as i32;
                let first = entity.events().entity_first_persisted_at();
                let id = &entity.id;
                let row = sqlx::query!(
                    #persist_snapshot_query,
                    id as &#id_type,
                    head,
                    <<#entity as es_entity::EsEntity>::Snapshot as es_entity::EsSnapshot>::FINGERPRINT,
                    snapshot_json,
                    first,
                    op.maybe_now(),
                )
                .fetch_optional(op.as_executor())
                .await?;
                match row {
                    Some(row) => {
                        #payload_code
                        entity.events_mut().compact_to_snapshot(
                            state,
                            row.recorded_at,
                            first.unwrap_or(row.recorded_at),
                        );
                        Ok(true)
                    }
                    None => Ok(false),
                }
            }
        }
    }
}
