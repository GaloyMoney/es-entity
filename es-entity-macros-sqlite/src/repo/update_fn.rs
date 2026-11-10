use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::{events_write::SnapshotUpsert, options::*};

pub struct UpdateFn<'a> {
    in_op_only: bool,
    entity: &'a syn::Ident,
    table_name: &'a str,
    events_table_name: &'a str,
    snapshot_table_name: Option<&'a str>,
    columns: &'a Columns,
    constraint_violation: syn::Ident,
    nested_fn_names: Vec<syn::Ident>,
    post_persist_hook: bool,
    /// Whether the generated `update`/`update_in_op` can ever reject — see
    /// `RepositoryOptions::update_can_reject`. `false` narrows every
    /// generated signature (and the inner `__result` annotation) from
    /// `RepoWriteError<CV>` to the plain `es_entity::RepoFault`.
    update_can_reject: bool,
    /// The generated signatures' error type: `update_can_reject` folded with
    /// every nested child's shape, see `RepositoryOptions::update_error_ty`.
    write_error_ty: TokenStream,
    #[cfg(feature = "instrument")]
    repo_name_snake: String,
}

impl<'a> From<&'a RepositoryOptions> for UpdateFn<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            in_op_only: opts.in_op_only(),
            entity: opts.entity(),
            constraint_violation: opts.constraint_violation(),
            columns: &opts.columns,
            table_name: opts.table_name(),
            events_table_name: opts.events_table_name(),
            snapshot_table_name: opts.snapshot_table_name(),
            nested_fn_names: opts
                .all_nested()
                .map(|f| f.update_nested_fn_name())
                .collect(),
            post_persist_hook: opts.post_persist_hook.is_some(),
            update_can_reject: opts.update_can_reject(),
            write_error_ty: opts.update_error_ty(),
            #[cfg(feature = "instrument")]
            repo_name_snake: opts.repo_name_snake_case(),
        }
    }
}

impl ToTokens for UpdateFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let entity = self.entity;
        let constraint_values = self.columns.constraint_values(entity, false);
        let constraint_violation = &self.constraint_violation;
        let table_name = self.table_name;
        let events_table_name = self.events_table_name;
        let write_error_ty = &self.write_error_ty;

        let (constraint_values_binding, reject_handling) = if self.update_can_reject {
            (
                constraint_values,
                quote! { .map_rejected(|r| r.with_attempted(__constraint_values)) },
            )
        } else {
            (quote! {}, quote! { .narrow_rejected() })
        };

        let nested = self.nested_fn_names.iter().map(|f| {
            quote! {
                self.#f(op, &mut [&mut *entity]).await.map_err(es_entity::LiftNestedWrite::<#constraint_violation>::lift_nested)?;
            }
        });

        // SQLite has no data-modifying CTEs: the index update (when there are
        // index columns to persist) runs first as its own statement — index
        // errors win, as on Postgres — and the events go through the shared
        // `persist_events`.
        let snapshot_upsert = self
            .snapshot_table_name
            .map(|table| SnapshotUpsert { table });

        let index_update = if self.columns.updates_needed() {
            let assignments = self
                .columns
                .variable_assignments_for_update(syn::parse_quote! { entity });
            let column_updates = self.columns.sql_updates();
            let args = self.columns.update_query_args();
            let update_query = crate::runtime_query::query(
                &format!(
                    "UPDATE {} SET {} WHERE id = $1 RETURNING id",
                    self.table_name, column_updates,
                ),
                &args,
            );

            quote! {
                use es_entity::errlanes::ResultExt as _;

                #assignments
                #constraint_values_binding

                let __updated = #update_query
                    .fetch_all(op.as_executor())
                    .await
                    .map_err(|e| Self::classify_update_write(e, format!("{} seq conflict", #table_name)))
                    #reject_handling?;

                // The UPDATE matched nothing: the index row was deleted
                // underneath us. Report that as a lost race, matching
                // `update_all`.
                let _ = __updated
                    .first()
                    .ok_or_else(|| errlanes::Transient::new(errlanes::TransientKind::OptimisticConflict)
                            .with_context(format!("{} row vanished", #table_name))
                    )?;
            }
        } else {
            quote! {}
        };

        let persist_tokens = if snapshot_upsert.is_some() {
            quote! {
                let __snapshot = <#entity as es_entity::HeadSnapshot>::capture(&*entity);
                // Recompute before the write moves new events into the
                // persisted tail.
                let __snapshot_first = Self::extract_events(entity).entity_first_persisted_at();
                let n_events = {
                    let events = Self::extract_events(entity);
                    Self::classify_conflict(
                        self.persist_events(op, events, __snapshot.as_ref()).await,
                        #events_table_name,
                        || format!("{} seq conflict", #table_name),
                    )?
                };
                // `persist_events` reports only the count, not the
                // `recorded_at` it stamped new events with — recover it from
                // the entity itself for `compact_to_snapshot` below.
                let recorded_at = Self::extract_events(entity)
                    .entity_last_modified_at()
                    .ok_or_else(|| errlanes::Transient::new(errlanes::TransientKind::OptimisticConflict)
                            .with_context(format!("{} row vanished", #table_name))
                    )?;
            }
        } else {
            quote! {
                let n_events = {
                    let events = Self::extract_events(entity);
                    Self::classify_conflict(
                        self.persist_events(op, events).await,
                        #events_table_name,
                        || format!("{} seq conflict", #table_name),
                    )?
                };
            }
        };

        let compact_code = match &snapshot_upsert {
            Some(su) => su.compact_per_entity(
                quote! { Self::extract_events(entity) },
                quote! { recorded_at },
            ),
            None => quote! {},
        };

        // A clean entity whose loaded state had no matching snapshot (a
        // fingerprint change, or one that predates `snapshot` altogether)
        // gets one chance to refresh here before the no-op early return.
        let stale_refresh = self.snapshot_table_name.map(|_| {
            quote! {
                if Self::extract_events(entity).snapshot().is_none() {
                    self.__persist_snapshot_in_op(op, entity).await?;
                }
            }
        });

        #[cfg(feature = "instrument")]
        let (instrument_attr, record_id, error_recording) = {
            use convert_case::{Case, Casing};

            let entity_name = entity.to_string();
            let repo_name = &self.repo_name_snake;

            let id_ident = quote::format_ident!("{}_id", entity.to_string().to_case(Case::Snake));

            let span_name = format!("{}.update", repo_name);
            (
                quote! {
                    #[tracing::instrument(name = #span_name, skip_all, fields(entity = #entity_name, #id_ident = tracing::field::Empty, error = tracing::field::Empty, exception.message = tracing::field::Empty, exception.type = tracing::field::Empty))]
                },
                quote! {
                    tracing::Span::current().record(stringify!(#id_ident), tracing::field::display(&entity.id));
                },
                quote! {
                    if let Err(ref e) = __result {
                        tracing::Span::current().record("error", true);
                        tracing::Span::current().record("exception.message", tracing::field::display(e));
                        tracing::Span::current().record("exception.type", std::any::type_name_of_val(e));
                    }
                },
            )
        };
        #[cfg(not(feature = "instrument"))]
        let (instrument_attr, record_id, error_recording) = (quote! {}, quote! {}, quote! {});

        let post_persist_check = if self.post_persist_hook {
            quote! {
                self.execute_post_persist_hook(op, &entity, entity.events().last_persisted(n_events)).await?;
            }
        } else {
            quote! {}
        };

        let standalone = (!self.in_op_only).then(|| {
            quote! {
                pub async fn update(
                    &self,
                    entity: &mut #entity
                ) -> Result<usize, #write_error_ty> {
                    let mut op = self.begin_op().await?;
                    let res = self.update_in_op(&mut op, entity).await?;
                    op.commit().await?;
                    Ok(res)
                }
            }
        });

        tokens.append_all(quote! {
            #[inline(always)]
            fn extract_events<Entity, Event, Snapshot>(entity: &mut Entity) -> &mut es_entity::EntityEvents<Event, Snapshot>
            where
                Entity: es_entity::EsEntity<Event = Event, Snapshot = Snapshot>,
                Event: es_entity::EsEvent,
                Snapshot: es_entity::EsSnapshot,
            {
                entity.events_mut()
            }

            #standalone

            #instrument_attr
            pub async fn update_in_op<OP>(
                &self,
                op: &mut OP,
                entity: &mut #entity
            ) -> Result<usize, #write_error_ty>
            where
                OP: es_entity::AtomicOperation + ?Sized
            {
                let __result: Result<usize, #write_error_ty> = async {
                    #record_id
                    #(#nested)*

                    if !Self::extract_events(entity).any_new() {
                        #stale_refresh
                        return Ok(0);
                    }

                    #index_update
                    #persist_tokens

                    #post_persist_check

                    #compact_code

                    Ok(n_events)
                }.await;

                #error_recording
                __result
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proc_macro2::Span;
    use syn::Ident;

    fn update_fn_tokens(columns: &Columns, snapshot_table_name: Option<&str>) -> String {
        let entity = Ident::new("Entity", Span::call_site());
        let update_fn = UpdateFn {
            in_op_only: false,
            entity: &entity,
            table_name: "entities",
            events_table_name: "entity_events",
            snapshot_table_name,
            constraint_violation: Ident::new("EntityConstraintViolation", Span::call_site()),
            columns,
            nested_fn_names: Vec::new(),
            post_persist_hook: false,
            update_can_reject: true,
            write_error_ty: quote! { es_entity::RepoWriteError<EntityConstraintViolation> },
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };
        let mut tokens = TokenStream::new();
        update_fn.to_tokens(&mut tokens);
        tokens.to_string()
    }

    /// The index update is its own SQLite statement (no data-modifying CTE)
    /// and runs before the events go through `persist_events`.
    #[test]
    fn update_fn_runs_index_update_then_persist_events() {
        let id = syn::parse_str("EntityId").unwrap();
        let columns = Columns::new(
            &id,
            [Column::new(
                Ident::new("name", Span::call_site()),
                syn::parse_str("String").unwrap(),
            )],
        );
        let out = update_fn_tokens(&columns, None);
        assert!(out.contains("\"UPDATE entities SET name = ?2 WHERE id = ?1 RETURNING id\""));
        assert!(!out.contains("WITH updated"));
        assert!(!out.contains("UNNEST"));
        let update_at = out.find("UPDATE entities").unwrap();
        let persist_at = out.find("persist_events").unwrap();
        assert!(update_at < persist_at);
    }

    #[test]
    fn update_fn_without_columns_only_persists_events() {
        let id = syn::parse_str("EntityId").unwrap();
        let mut columns = Columns::default();
        columns.set_id_column(&id);
        let out = update_fn_tokens(&columns, None);
        assert!(!out.contains("UPDATE entities"));
        assert!(out.contains("self . persist_events (op , events) . await"));
    }

    #[test]
    fn update_fn_with_snapshot_passes_the_capture_and_compacts() {
        let id = syn::parse_str("EntityId").unwrap();
        let mut columns = Columns::default();
        columns.set_id_column(&id);
        let out = update_fn_tokens(&columns, Some("entity_snapshots"));
        assert!(out.contains("persist_events (op , events , __snapshot . as_ref ())"));
        assert!(out.contains("compact_to_snapshot"));
        assert!(out.contains("__persist_snapshot_in_op"));
    }
}
