use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::{
    events_write::{EventSource, EventsInsert},
    options::*,
};

pub struct ForgetFn<'a> {
    in_op_only: bool,
    id: &'a syn::Ident,
    entity: &'a syn::Ident,
    event: &'a syn::Ident,
    constraint_violation: syn::Ident,
    table_name: &'a str,
    events_table_name: &'a str,
    event_ctx: bool,
    forgettable_table_name: &'a str,
    forgettable_columns: Vec<&'a syn::Ident>,
    snapshot_table_name: Option<&'a str>,
    post_persist_hook: bool,
}

impl<'a> ForgetFn<'a> {
    pub fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            in_op_only: opts.in_op_only(),
            id: opts.id(),
            entity: opts.entity(),
            event: opts.event(),
            constraint_violation: opts.constraint_violation(),
            table_name: opts.table_name(),
            events_table_name: opts.events_table_name(),
            event_ctx: opts.event_context_enabled(),
            forgettable_table_name: opts
                .forgettable_table_name()
                .expect("forgettable must be enabled"),
            forgettable_columns: opts.columns.forgettable_column_names(),
            snapshot_table_name: opts.snapshot_table_name(),
            post_persist_hook: opts.post_persist_hook.is_some(),
        }
    }
}

impl ToTokens for ForgetFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let id_type = &self.id;
        let entity_type = self.entity;
        let event_type = self.event;
        let constraint_violation = &self.constraint_violation;
        let constraint_values = quote::format_ident!("{}ConstraintValues", entity_type);
        let table_name = self.table_name;
        let events_table_name = self.events_table_name;

        let query = format!(
            "DELETE FROM {} WHERE entity_id = $1",
            self.forgettable_table_name
        );

        // Also NULL any `Forgettable<..>` index columns so the materialised
        // lookup table stops exposing the forgotten value. When there are such
        // columns, that UPDATE and the staged-event insert go out as one
        // statement; otherwise there is nothing to combine and the shared
        // `persist_events` is used.
        //
        // The payload delete deliberately stays a *separate, later* statement:
        // sub-statements of a data-modifying CTE cannot see each other's
        // writes, so folding it in would let payload rows written by the same
        // statement survive the erasure. Combining also makes the staged
        // payload insert unnecessary — inserting rows the delete would remove
        // in the same transaction is unobservable — so the combined path skips
        // it, and a staged event still cannot smuggle a value past erasure.
        //
        // Either way the post-persist hook is handed exactly the events this
        // call persisted, so the count has to be captured — but only when a
        // hook exists, to keep an unused binding out of the generated code.
        // On the `persist_events` path that call reports it; on the combined
        // path it comes from marking the events, after the payload delete.
        let wants_hook = self.post_persist_hook;
        // `forget` never snapshots the events it stages: the snapshot forced
        // to `None` disables the CTE's `WHERE … IS NOT NULL` guard, so
        // nothing gets written here — the rebuild-and-re-snapshot steps
        // below produce the real, forgotten snapshot afterward.
        let persist_events_snapshot_arg = self.snapshot_table_name.map(|_| quote! { , None });

        let (persist_staged, count_persisted) = if self.forgettable_columns.is_empty() {
            let persist = if wants_hook {
                quote! {
                    let n_events = if entity.events().any_new() {
                        Self::classify_conflict(
                            self.persist_events(op, entity.events_mut() #persist_events_snapshot_arg).await,
                            #events_table_name,
                            || format!("{} seq conflict", #table_name),
                        )?
                    } else {
                        0
                    };
                }
            } else {
                quote! {
                    if entity.events().any_new() {
                        Self::classify_conflict(
                            self.persist_events(op, entity.events_mut() #persist_events_snapshot_arg).await,
                            #events_table_name,
                            || format!("{} seq conflict", #table_name),
                        )?;
                    }
                }
            };
            // `persist_events` already reported the count.
            (persist, quote! {})
        } else {
            let set_clause = self
                .forgettable_columns
                .iter()
                .map(|c| format!("{} = NULL", c))
                .collect::<Vec<_>>()
                .join(", ");
            let events_insert = EventsInsert::new(self.events_table_name, self.event_ctx);
            let source = EventSource::PerEntityCte {
                cte: "updated",
                offset_param: Some(3),
            };
            let combined_query = format!(
                "WITH updated AS (UPDATE {} SET {} WHERE id = $1 RETURNING id) {}",
                self.table_name,
                set_clause,
                events_insert.sql(&source, 2, 4),
            );

            let gather = events_insert.gather_per_entity(quote! { entity.events() });
            let event_args = events_insert.arg_exprs(&source);

            let persist = quote! {
                let has_new_events = entity.events().any_new();
                #gather

                let rows = {
                    let id = &entity.id;
                    sqlx::query!(
                        #combined_query,
                        id as &#id_type,
                        #(#event_args),*
                    )
                    .fetch_all(op.as_executor())
                    .await
                    .map_err(|e| Self::classify_update_write(e, format!("{} seq conflict", #table_name)).map_rejected(|r| r.with_attempted(#constraint_values { id: Some((*id).clone()), ..Default::default() })))?
                };
            };

            // Marking needs the events mutably, so it waits until the payload
            // delete has had its turn with `&entity.id`. No row means the
            // CTE's UPDATE matched nothing — the entity was hard-deleted
            // underneath us; report the lost race, not an internal error.
            let recorded_at = quote! {
                let recorded_at = rows
                    .first()
                    .map(|row| row.recorded_at)
                    .ok_or_else(|| errlanes::Fail::from(
                        errlanes::Transient::new(errlanes::TransientKind::OptimisticConflict)
                            .with_context(format!("{} row vanished", #table_name))
                    ))?;
            };
            let count = if wants_hook {
                quote! {
                    let n_events = if has_new_events {
                        #recorded_at
                        entity.events_mut().mark_new_events_persisted_at(recorded_at)
                    } else {
                        0
                    };
                }
            } else {
                quote! {
                    if has_new_events {
                        #recorded_at
                        entity.events_mut().mark_new_events_persisted_at(recorded_at);
                    }
                }
            };
            (persist, count)
        };

        let post_persist_check = if wants_hook {
            quote! {
                if n_events > 0 {
                    self.execute_post_persist_hook(
                        op,
                        &entity,
                        entity.events().last_persisted(n_events)
                    ).await?;
                }
            }
        } else {
            quote! {}
        };

        let (delete_snapshot_row, rebuild, re_snapshot) = match self.snapshot_table_name {
            Some(snapshot_tbl) => {
                let delete_query = format!("DELETE FROM {snapshot_tbl} WHERE id = $1");
                (
                    quote! {
                        sqlx::query!(#delete_query, &entity.id as &#id_type)
                            .execute(op.as_executor())
                            .await?;
                    },
                    quote! {
                        // A `Fatal(Invariant)` carrying `NotFound` here means the
                        // entity was hard-deleted underneath the erasure — a lost
                        // race, not a bug, unlike a `find_by_id` miss anywhere
                        // else.
                        let mut entity: #entity_type = match self
                            .__full_history_find_by_id_in_op(&mut *op, &entity.id)
                            .await
                        {
                            Ok(e) => e,
                            Err(es_entity::RepoFault::Fatal(fatal)) if es_entity::fatal_is_not_found(&fatal) => {
                                return Err(errlanes::Fail::from(
                                    errlanes::Transient::new(errlanes::TransientKind::OptimisticConflict)
                                        .with_context(format!("{} vanished during forget", #table_name))
                                ));
                            }
                            Err(other) => return Err(other.into()),
                        };
                    },
                    quote! {
                        self.__persist_snapshot_in_op(op, &mut entity).await?;
                    },
                )
            }
            None => (
                quote! {},
                quote! {
                    let events = entity.events_mut().forget_and_take(
                        #event_type::forget_forgettable_payloads
                    );
                    let entity: #entity_type = es_entity::TryFromEvents::try_from_events(events)?;
                },
                quote! {},
            ),
        };

        let standalone = (!self.in_op_only).then(|| {
            quote! {
                /// Permanently forgets the entity's forgettable data. Consumes the
                /// entity and returns the rebuilt (forgotten) entity. On any error
                /// the potentially-inconsistent copy is dropped — reload and retry.
                pub async fn forget(
                    &self,
                    entity: #entity_type
                ) -> Result<#entity_type, es_entity::RepoWriteError<#constraint_violation>> {
                    let mut op = self.begin_op().await?;
                    let entity = self.forget_in_op(&mut op, entity).await?;
                    op.commit().await?;
                    Ok(entity)
                }
            }
        });

        tokens.append_all(quote! {
            #standalone

            /// Permanently forgets the entity's forgettable data — all in one
            /// transaction: persists any staged (unpersisted) events, deletes
            /// all payload rows, NULLs forgettable index columns, and rebuilds
            /// the entity from the drained events.
            ///
            /// Consumes the entity by value and returns the rebuilt (forgotten)
            /// entity; on any error the consumed copy is dropped, so no
            /// half-mutated entity can survive a failed erasure.
            ///
            /// Staged events are persisted **before** the payload delete:
            /// payload rows their persistence inserts are hard-deleted in the
            /// same transaction, so a staged event can never smuggle a raw
            /// forgettable value past the erasure. Persisting them also
            /// consumes sequence numbers — the concurrency fence. By
            /// convention, stage a domain erasure event (e.g. an empty
            /// `Forgot {}`) before calling `forget`: stale copies that
            /// `update()` afterwards then fail with `ConcurrentModification`,
            /// and the erasure is recorded in the event stream. **Without a
            /// staged event no sequence is consumed and a stale writer can
            /// re-persist the forgotten data** — see the book chapter.
            ///
            /// `forget` itself can fail with `ConcurrentModification` if
            /// another writer got there first — reload and re-forget (repeat
            /// forgets are legitimate).
            ///
            /// If the repository configures a `post_persist_hook`, it runs for
            /// the staged events persisted by this call — exactly once, after
            /// the payload delete and entity rebuild, so the hook observes the
            /// forgotten representation (never the raw payloads being erased)
            /// while still running inside the erasure transaction. When no
            /// staged events are persisted the hook is not invoked, matching
            /// `update`'s no-op semantics.
            pub async fn forget_in_op<OP>(
                &self,
                op: &mut OP,
                mut entity: #entity_type
            ) -> Result<#entity_type, es_entity::RepoWriteError<#constraint_violation>>
            where
                OP: es_entity::AtomicOperation + ?Sized
            {
                #persist_staged
                {
                    let id = &entity.id;
                    sqlx::query!(
                        #query,
                        id as &#id_type
                    )
                    .execute(op.as_executor())
                    .await?;
                }
                #count_persisted
                #delete_snapshot_row
                #rebuild
                #post_persist_check
                #re_snapshot
                Ok(entity)
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proc_macro2::Span;
    use syn::Ident;

    #[test]
    fn forget_fn() {
        let id = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let event = Ident::new("EntityEvent", Span::call_site());

        let forget_fn = ForgetFn {
            in_op_only: false,
            id: &id,
            entity: &entity,
            event: &event,
            constraint_violation: Ident::new("EntityConstraintViolation", Span::call_site()),
            table_name: "entities",
            events_table_name: "entity_events",
            event_ctx: false,
            forgettable_table_name: "entities_forgettable_payloads",
            forgettable_columns: Vec::new(),
            snapshot_table_name: None,
            post_persist_hook: false,
        };

        let mut tokens = TokenStream::new();
        forget_fn.to_tokens(&mut tokens);

        let output = tokens.to_string();
        // Consume-and-return: forget takes the entity by value and returns the
        // rebuilt (forgotten) entity — no `&mut`, no in-place assignment.
        assert!(output.contains(
            "entity : Entity) -> Result < Entity , es_entity :: RepoWriteError < EntityConstraintViolation >>"
        ));
        assert!(!output.contains("& mut Entity"));
        assert!(!output.contains("* entity ="));
        assert!(output.contains("es_entity :: TryFromEvents :: try_from_events"));
        assert!(output.contains("Ok (entity)"));
        // No hook configured — no hook invocation is generated.
        assert!(!output.contains("execute_post_persist_hook"));
        // Staged events are persisted (fencing + no laundering), BEFORE the
        // payload delete — assert the persist appears before the DELETE.
        let persist_at = output
            .find("persist_events")
            .expect("staged events must be persisted");
        let delete_at = output
            .find("DELETE FROM entities_forgettable_payloads WHERE entity_id = $1")
            .expect("payload delete present");
        assert!(persist_at < delete_at, "must persist BEFORE payload delete");
        assert!(output.contains("Self :: classify_conflict"));
        // No framework-appended marker: erasure events are a client convention.
        assert!(!output.contains(":: Forgot"));
        assert!(output.contains("forget_and_take (EntityEvent :: forget_forgettable_payloads)"));
    }

    #[test]
    fn forget_fn_nulls_index_columns() {
        let id = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let event = Ident::new("EntityEvent", Span::call_site());
        let email = Ident::new("email", Span::call_site());

        let forget_fn = ForgetFn {
            in_op_only: false,
            id: &id,
            entity: &entity,
            event: &event,
            constraint_violation: Ident::new("EntityConstraintViolation", Span::call_site()),
            table_name: "entities",
            events_table_name: "entity_events",
            event_ctx: false,
            forgettable_table_name: "entities_forgettable_payloads",
            forgettable_columns: vec![&email],
            snapshot_table_name: None,
            post_persist_hook: false,
        };

        let mut tokens = TokenStream::new();
        forget_fn.to_tokens(&mut tokens);

        let output = tokens.to_string();
        // The index-column NULLing is now the CTE of the combined statement
        // that also inserts any staged events.
        assert!(output.contains(
            "WITH updated AS (UPDATE entities SET email = NULL WHERE id = $1 RETURNING id) INSERT INTO entity_events"
        ));
        // No laundering: the payload delete must still come after the statement
        // that persists staged events, and must not be folded into it (CTE
        // sub-statements cannot see each other's writes).
        let insert_at = output
            .find("INSERT INTO entity_events")
            .expect("staged events must be persisted");
        let delete_at = output
            .find("DELETE FROM entities_forgettable_payloads WHERE entity_id = $1")
            .expect("payload delete present");
        assert!(insert_at < delete_at, "must persist BEFORE payload delete");
        // The combined path skips the staged payload insert entirely — the
        // delete would remove those rows in the same transaction anyway.
        assert!(!output.contains("INSERT INTO entities_forgettable_payloads"));
    }

    #[test]
    fn forget_fn_runs_post_persist_hook() {
        let id = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let event = Ident::new("EntityEvent", Span::call_site());

        let forget_fn = ForgetFn {
            in_op_only: false,
            id: &id,
            entity: &entity,
            event: &event,
            constraint_violation: Ident::new("EntityConstraintViolation", Span::call_site()),
            table_name: "entities",
            events_table_name: "entity_events",
            event_ctx: false,
            forgettable_table_name: "entities_forgettable_payloads",
            forgettable_columns: Vec::new(),
            snapshot_table_name: None,
            post_persist_hook: true,
        };

        let mut tokens = TokenStream::new();
        forget_fn.to_tokens(&mut tokens);

        let output = tokens.to_string();
        // The hook runs once, with the just-persisted staged events...
        assert!(output.contains("if n_events > 0"));
        assert!(
            output.contains(
                "self . execute_post_persist_hook (op , & entity , entity . events () . last_persisted (n_events))"
            ),
            "hook must receive exactly the just-persisted events: {output}"
        );
        // ...AFTER the rebuild (hook observes the forgotten representation):
        // the hook invocation must come after try_from_events.
        let rebuild_at = output.find("try_from_events").expect("rebuild present");
        let hook_at = output
            .find("execute_post_persist_hook")
            .expect("hook invocation present");
        assert!(rebuild_at < hook_at, "hook must run on the rebuilt entity");
    }

    /// The combined path does not route through `persist_events`, so it has to
    /// report the persisted-event count itself — from marking the events, which
    /// happens after the payload delete — for the hook to receive them.
    #[test]
    fn forget_fn_runs_post_persist_hook_on_the_combined_path() {
        let id = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let event = Ident::new("EntityEvent", Span::call_site());
        let email = Ident::new("email", Span::call_site());

        let forget_fn = ForgetFn {
            in_op_only: false,
            id: &id,
            entity: &entity,
            event: &event,
            constraint_violation: Ident::new("EntityConstraintViolation", Span::call_site()),
            table_name: "entities",
            events_table_name: "entity_events",
            event_ctx: false,
            forgettable_table_name: "entities_forgettable_payloads",
            forgettable_columns: vec![&email],
            snapshot_table_name: None,
            post_persist_hook: true,
        };

        let mut tokens = TokenStream::new();
        forget_fn.to_tokens(&mut tokens);

        let output = tokens.to_string();
        // The count comes from marking, not from `persist_events`.
        assert!(!output.contains("persist_events"));
        assert!(
            output.contains("let n_events = if has_new_events { let recorded_at = rows . first ()")
        );
        assert!(output.contains("mark_new_events_persisted_at (recorded_at) } else { 0 } ;"));
        // Same guarantees as the `persist_events` path: hook after the rebuild,
        // and only when events were actually persisted.
        assert!(output.contains("if n_events > 0"));
        let rebuild_at = output.find("try_from_events").expect("rebuild present");
        let hook_at = output
            .find("execute_post_persist_hook")
            .expect("hook invocation present");
        assert!(rebuild_at < hook_at, "hook must run on the rebuilt entity");
        // The count is taken after the payload delete, since marking needs the
        // events mutably while the delete still holds `&entity.id`.
        let delete_at = output
            .find("DELETE FROM entities_forgettable_payloads")
            .expect("payload delete present");
        let mark_at = output
            .find("mark_new_events_persisted_at")
            .expect("mark present");
        assert!(
            delete_at < mark_at,
            "marking must follow the payload delete"
        );
    }
}
