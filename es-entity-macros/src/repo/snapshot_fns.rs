use proc_macro2::TokenStream;
use quote::quote;

use super::options::*;

/// Emitted only for `#[es_repo(snapshot)]` repos: `full_history()` (bypasses
/// snapshots by binding `NO_SNAPSHOT_FINGERPRINT`), `persist_snapshot_in_op`
/// (backfill / post-forget re-snapshot), and `verify_snapshot[_in_op]` (the
/// equivalence test: full-history fold == snapshotted fold).
///
/// `full_history()` covers `find_by_id` / `maybe_find_by_id` only — the
/// handoff scopes it to the id column; `full_history()` on scoped repo
/// views and `full_history()` variants of `list_*` are out of scope.
///
/// `es_query!`'s expansion refers to `Self` (as `EsRepo`), so the actual
/// queries must live inside `impl #repo { .. }` where `Self` is the repo —
/// not inside `impl FooFullHistory { .. }`. The wrapper type's methods
/// (`outer_tokens`) purely delegate to private helpers emitted alongside
/// `full_history()` itself (`in_impl_tokens`).
pub struct SnapshotFns<'a> {
    in_op_only: bool,
    repo_ident: &'a syn::Ident,
    entity: &'a syn::Ident,
    id: &'a syn::Ident,
    table_name: &'a str,
    events_table_name: &'a str,
    snapshot_table_name: &'a str,
    forgettable_table_name: Option<&'a str>,
    column_enum: syn::Ident,
    find_error: syn::Ident,
    modify_error: syn::Ident,
    full_history_ident: syn::Ident,
}

impl<'a> SnapshotFns<'a> {
    pub fn from(opts: &'a RepositoryOptions) -> Option<Self> {
        let snapshot_table_name = opts.snapshot_table_name()?;
        Some(Self {
            in_op_only: opts.in_op_only(),
            repo_ident: &opts.ident,
            entity: opts.entity(),
            id: opts.id(),
            table_name: opts.table_name(),
            events_table_name: opts.events_table_name(),
            snapshot_table_name,
            forgettable_table_name: opts.forgettable_table_name(),
            column_enum: opts.column_enum(),
            find_error: opts.find_error(),
            modify_error: opts.modify_error(),
            full_history_ident: syn::Ident::new(
                &format!("{}FullHistory", opts.entity()),
                proc_macro2::Span::call_site(),
            ),
        })
    }

    /// `full_history()` (+ its private query helpers), `persist_snapshot_in_op`,
    /// `verify_snapshot[_in_op]` — go inside `impl #repo { .. }`.
    pub fn in_impl_tokens(&self) -> TokenStream {
        let entity = self.entity;
        let id_type = self.id;
        let modify_error = &self.modify_error;
        let find_error = &self.find_error;
        let column_enum = &self.column_enum;
        let full_history = &self.full_history_ident;
        let snapshot_tbl = self.snapshot_table_name;
        let table_name = self.table_name;
        let events_table_name = self.events_table_name;
        let entity_name = entity.to_string();

        let forgettable_tbl_arg = match self.forgettable_table_name {
            Some(tbl) => quote! { forgettable_tbl = #tbl, },
            None => quote! {},
        };
        let find_by_id_sql = format!("SELECT id FROM {table_name} WHERE id = $1");
        let query_call = quote! {
            es_entity::es_query!(
                entity = #entity,
                #forgettable_tbl_arg
                snapshot_tbl = #snapshot_tbl,
                snapshot_fingerprint = es_entity::NO_SNAPSHOT_FINGERPRINT,
                #find_by_id_sql,
                id as &#id_type,
            )
        };

        let persist_snapshot_query = format!(
            "INSERT INTO {snapshot_tbl} (id, sequence, fingerprint, snapshot, first_recorded_at, recorded_at) \
             SELECT $1, $2, $3, $4, COALESCE($5, COALESCE($6, NOW())), COALESCE($6, NOW()) \
             WHERE (SELECT MAX(sequence) FROM {events_table_name} WHERE id = $1) = $2 \
             ON CONFLICT (id) DO UPDATE SET sequence = EXCLUDED.sequence, fingerprint = EXCLUDED.fingerprint, \
             snapshot = EXCLUDED.snapshot, first_recorded_at = EXCLUDED.first_recorded_at, recorded_at = EXCLUDED.recorded_at \
             WHERE EXCLUDED.sequence > {snapshot_tbl}.sequence \
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
            /// A view of this repo whose loaders bypass snapshots: every
            /// entity is loaded from its full event history. For audits,
            /// debugging, and `verify_snapshot`.
            pub fn full_history(&self) -> #full_history<'_> {
                #full_history(self)
            }

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

            async fn __full_history_maybe_find_by_id_in_op<'a, OP>(
                &self,
                op: OP,
                id: &#id_type,
            ) -> Result<Option<#entity>, #find_error>
            where
                OP: es_entity::IntoOneTimeExecutor<'a>,
            {
                Ok(#query_call.fetch_optional(op).await?)
            }

            /// Writes a snapshot with no new events: `snapshot()`'s result as
            /// of the entity's current head, guarded so a concurrent writer
            /// (including a `forget` that staged its own erasure event)
            /// makes this a no-op rather than an error.
            ///
            /// Errors if `entity` has unpersisted staged events — persist
            /// them first.
            pub async fn persist_snapshot_in_op<OP>(
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
                let state = match <#entity as es_entity::Snapshotting>::snapshot(&*entity) {
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

            /// Loads `id` both via its snapshot and via `full_history()`, and
            /// compares the two folds. `Ok(())` means the snapshot is a
            /// faithful summary; `Err(SnapshotMismatch)` means some scan or
            /// `idempotency_guard!` clause is not accounting for the
            /// snapshot correctly.
            pub async fn verify_snapshot_in_op<OP>(
                &self,
                op: &mut OP,
                id: impl std::borrow::Borrow<#id_type>,
            ) -> Result<(), #find_error>
            where
                OP: es_entity::AtomicOperation + ?Sized,
            {
                let id = id.borrow();
                let full = self.__full_history_find_by_id_in_op(&mut *op, id).await?;
                let snapshotted = self.find_by_id_in_op(&mut *op, id).await?;
                let full_snapshot = <#entity as es_entity::Snapshotting>::snapshot(&full);
                let snapshotted_snapshot = <#entity as es_entity::Snapshotting>::snapshot(&snapshotted);
                if full_snapshot == snapshotted_snapshot {
                    Ok(())
                } else {
                    Err(#find_error::SnapshotMismatch(es_entity::SnapshotMismatch {
                        full_history: format!("{full_snapshot:?}"),
                        snapshotted: format!("{snapshotted_snapshot:?}"),
                    }))
                }
            }

            /// Standalone form of [`verify_snapshot_in_op`](Self::verify_snapshot_in_op).
            pub async fn verify_snapshot(
                &self,
                id: impl std::borrow::Borrow<#id_type>,
            ) -> Result<(), #find_error> {
                let mut op = self.begin_op().await?;
                self.verify_snapshot_in_op(&mut op, id).await
            }
        }
    }

    /// The `{Entity}FullHistory<'r>` wrapper type and its public loaders —
    /// go at the top level, alongside the repo's `impl` block. Pure
    /// delegation to the private helpers in `in_impl_tokens`.
    pub fn outer_tokens(&self) -> TokenStream {
        let repo_ident = self.repo_ident;
        let entity = self.entity;
        let id_type = self.id;
        let find_error = &self.find_error;
        let full_history = &self.full_history_ident;

        let standalone = (!self.in_op_only).then(|| {
            quote! {
                impl #full_history<'_> {
                    pub async fn find_by_id(
                        &self,
                        id: impl std::borrow::Borrow<#id_type>,
                    ) -> Result<#entity, #find_error> {
                        self.0.__full_history_find_by_id_in_op(self.0.pool(), id.borrow()).await
                    }

                    pub async fn maybe_find_by_id(
                        &self,
                        id: impl std::borrow::Borrow<#id_type>,
                    ) -> Result<Option<#entity>, #find_error> {
                        self.0.__full_history_maybe_find_by_id_in_op(self.0.pool(), id.borrow()).await
                    }
                }
            }
        });

        let doc = format!(
            "A view of a [`{repo_ident}`] whose loaders bypass snapshots. Obtained via [`{repo_ident}::full_history`]."
        );

        quote! {
            #[doc = #doc]
            pub struct #full_history<'r>(&'r #repo_ident);

            #standalone

            impl #full_history<'_> {
                pub async fn find_by_id_in_op<'a, OP>(
                    &self,
                    op: OP,
                    id: impl std::borrow::Borrow<#id_type>,
                ) -> Result<#entity, #find_error>
                where
                    OP: es_entity::IntoOneTimeExecutor<'a>,
                {
                    self.0.__full_history_find_by_id_in_op(op, id.borrow()).await
                }

                pub async fn maybe_find_by_id_in_op<'a, OP>(
                    &self,
                    op: OP,
                    id: impl std::borrow::Borrow<#id_type>,
                ) -> Result<Option<#entity>, #find_error>
                where
                    OP: es_entity::IntoOneTimeExecutor<'a>,
                {
                    self.0.__full_history_maybe_find_by_id_in_op(op, id.borrow()).await
                }
            }
        }
    }
}
