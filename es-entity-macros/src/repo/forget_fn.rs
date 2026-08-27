use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::{
    error_classifier::concurrent_modification_classifier,
    events_write::{EventSource, EventsInsert},
    options::*,
};

pub struct ForgetFn<'a> {
    id: &'a syn::Ident,
    entity: &'a syn::Ident,
    event: &'a syn::Ident,
    error: syn::Ident,
    table_name: &'a str,
    events_table_name: &'a str,
    event_ctx: bool,
    forgettable_table_name: &'a str,
    forgettable_columns: Vec<&'a syn::Ident>,
    nested_forget_fn_names: Vec<syn::Ident>,
    post_persist_error: Option<&'a syn::Type>,
}

impl<'a> ForgetFn<'a> {
    pub fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            id: opts.id(),
            entity: opts.entity(),
            event: opts.event(),
            error: opts.forget_error(),
            table_name: opts.table_name(),
            events_table_name: opts.events_table_name(),
            event_ctx: opts.event_context_enabled(),
            forgettable_table_name: opts
                .forgettable_table_name()
                .expect("forgettable must be enabled"),
            forgettable_columns: opts.columns.forgettable_column_names(),
            nested_forget_fn_names: opts
                .all_nested()
                .map(|f| f.forget_nested_fn_name())
                .collect(),
            post_persist_error: opts.post_persist_hook.as_ref().map(|h| &h.error),
        }
    }
}

impl ToTokens for ForgetFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let id_type = &self.id;
        let entity_type = self.entity;
        let event_type = self.event;
        let error = &self.error;

        // Descends into every nested field's direct children, mirroring
        // `delete`'s own cascade: scrub by parent id, not by replaying each
        // child's event stream. Runs before the root's own payload delete —
        // order doesn't matter for the smuggling hazard `#persist_staged`
        // guards against (this cascade only ever removes child payload rows,
        // it never creates one), but matching delete's cascade-then-own-row
        // order keeps the two write paths easy to compare.
        let nested_forgets = self.nested_forget_fn_names.iter().map(|f| {
            quote! {
                Self::#f::<_, _, #error>(op, &entity).await?;
            }
        });

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
        let wants_hook = self.post_persist_error.is_some();

        let (persist_staged, count_persisted) = if self.forgettable_columns.is_empty() {
            let persist = if wants_hook {
                quote! {
                    let n_events = if entity.events().any_new() {
                        Self::extract_concurrent_modification(
                            self.persist_events(op, entity.events_mut()).await,
                            #error::ConcurrentModification,
                        )?
                    } else {
                        0
                    };
                }
            } else {
                quote! {
                    if entity.events().any_new() {
                        Self::extract_concurrent_modification(
                            self.persist_events(op, entity.events_mut()).await,
                            #error::ConcurrentModification,
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

            let classifier = concurrent_modification_classifier(error, self.events_table_name);
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
                    .map_err(#classifier)?
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
                    .ok_or(#error::ConcurrentModification)?;
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
                    ).await.map_err(#error::PostPersistHookError)?;
                }
            }
        } else {
            quote! {}
        };

        tokens.append_all(quote! {
            /// Permanently forgets the entity's forgettable data. Consumes the
            /// entity and returns the rebuilt (forgotten) entity. On any error
            /// the potentially-inconsistent copy is dropped — reload and retry.
            pub async fn forget(
                &self,
                entity: #entity_type
            ) -> Result<#entity_type, #error> {
                let mut op = self.begin_op().await?;
                let entity = self.forget_in_op(&mut op, entity).await?;
                op.commit().await?;
                Ok(entity)
            }

            /// Permanently forgets the entity's forgettable data — all in one
            /// transaction: persists any staged (unpersisted) events, deletes
            /// all payload rows, NULLs forgettable index columns, and rebuilds
            /// the entity from the drained events.
            ///
            /// Descends into every nested field's direct children too,
            /// mirroring `delete`'s own cascade: each child's forgettable
            /// payloads and index columns are scrubbed by parent id, the same
            /// way `delete` cascades its soft-delete flag. This does **not**
            /// replay or persist a child's own staged events — only `update`
            /// does that — so a child event staged before calling `forget` on
            /// its parent is left unpersisted, exactly as it would be if
            /// `forget` were never called.
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
            ) -> Result<#entity_type, #error>
            where
                OP: es_entity::AtomicOperation
            {
                #persist_staged
                #(#nested_forgets)*
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
                let events = entity.events_mut().forget_and_take(
                    #event_type::forget_forgettable_payloads
                );
                let entity: #entity_type = es_entity::TryFromEvents::try_from_events(events)?;
                #post_persist_check
                Ok(entity)
            }
        });

        self.verify_forgotten_tokens(tokens);
    }
}

impl ForgetFn<'_> {
    /// Generates `verify_forgotten` / `verify_forgotten_in_op`: a storage-level
    /// check that all configured forgettable data for an entity is physically
    /// absent — payload rows deleted, `Forgettable<..>` index columns NULL, and
    /// (defense-in-depth) no forgettable field holding a non-null value in the
    /// durable event JSON.
    fn verify_forgotten_tokens(&self, tokens: &mut TokenStream) {
        let id_type = &self.id;
        let event_type = self.event;
        let error = &self.error;

        let payload_count_query = format!(
            "SELECT COUNT(*) AS \"count!\" FROM {} WHERE entity_id = $1",
            self.forgettable_table_name
        );

        let event_fields_query = format!(
            "SELECT DISTINCT e.event_type AS \"event_type!\", f.field AS \"field!\" \
             FROM {} e \
             JOIN (SELECT UNNEST($2::text[]) AS event_type, UNNEST($3::text[]) AS field) f \
             ON e.event_type = f.event_type \
             WHERE e.id = $1 \
             AND e.event -> f.field IS NOT NULL \
             AND e.event -> f.field != 'null'::jsonb",
            self.events_table_name
        );

        let check_columns = if self.forgettable_columns.is_empty() {
            quote! {}
        } else {
            let selects = self
                .forgettable_columns
                .iter()
                .map(|c| format!("{c} IS NOT NULL AS \"{c}!\""))
                .collect::<Vec<_>>()
                .join(", ");
            let columns_query =
                format!("SELECT {} FROM {} WHERE id = $1", selects, self.table_name);
            let column_checks = self.forgettable_columns.iter().map(|c| {
                let name = c.to_string();
                quote! {
                    if row.#c {
                        remnants.live_index_columns.push(#name);
                    }
                }
            });
            quote! {
                if let Some(row) = sqlx::query!(
                    #columns_query,
                    id as &#id_type
                )
                .fetch_optional(op.as_executor())
                .await?
                {
                    #(#column_checks)*
                }
            }
        };

        tokens.append_all(quote! {
            /// Verifies at the **storage level** that all configured
            /// forgettable data for `id` is physically absent — i.e. that
            /// `forget()` has fully taken effect. Unlike inspecting a hydrated
            /// entity (which merely reads back as forgotten), this checks the
            /// database directly:
            ///
            /// 1. no rows remain in the forgettable payloads table,
            /// 2. all `Forgettable<..>` index columns are NULL, and
            /// 3. no forgettable field holds a non-null value in the durable
            ///    event JSON (defense-in-depth: the framework always writes
            ///    `null` there, so a hit indicates out-of-band writes).
            ///
            /// Returns `Err(NotForgotten(remnants))` describing anything still
            /// present. An entity that was never persisted verifies trivially.
            pub async fn verify_forgotten(
                &self,
                id: impl std::borrow::Borrow<#id_type>
            ) -> Result<(), #error> {
                let mut op = self.begin_op().await?;
                let res = self.verify_forgotten_in_op(&mut op, id).await?;
                op.commit().await?;
                Ok(res)
            }

            /// Same as [`Self::verify_forgotten`] but runs on an existing
            /// operation, so the check can share the erasure (or a follow-up)
            /// transaction.
            pub async fn verify_forgotten_in_op<OP>(
                &self,
                op: &mut OP,
                id: impl std::borrow::Borrow<#id_type>
            ) -> Result<(), #error>
            where
                OP: es_entity::AtomicOperation
            {
                let id = id.borrow();
                let mut remnants = es_entity::ForgettableRemnants::default();

                let payload_rows = sqlx::query!(
                    #payload_count_query,
                    id as &#id_type
                )
                .fetch_one(op.as_executor())
                .await?
                .count;
                remnants.payload_rows = payload_rows as usize;

                #check_columns

                let event_types: Vec<String> = #event_type::FORGETTABLE_JSON_FIELDS
                    .iter()
                    .map(|(t, _)| t.to_string())
                    .collect();
                let fields: Vec<String> = #event_type::FORGETTABLE_JSON_FIELDS
                    .iter()
                    .map(|(_, f)| f.to_string())
                    .collect();
                let rows = sqlx::query!(
                    #event_fields_query,
                    id as &#id_type,
                    &event_types[..],
                    &fields[..]
                )
                .fetch_all(op.as_executor())
                .await?;
                remnants.event_fields = rows
                    .into_iter()
                    .map(|r| (r.event_type, r.field))
                    .collect();

                if remnants.is_empty() {
                    Ok(())
                } else {
                    Err(#error::NotForgotten(remnants))
                }
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
        let error = Ident::new("EntityForgetError", Span::call_site());

        let forget_fn = ForgetFn {
            id: &id,
            entity: &entity,
            event: &event,
            error,
            table_name: "entities",
            events_table_name: "entity_events",
            event_ctx: false,
            forgettable_table_name: "entities_forgettable_payloads",
            forgettable_columns: Vec::new(),
            nested_forget_fn_names: Vec::new(),
            post_persist_error: None,
        };

        let mut tokens = TokenStream::new();
        forget_fn.to_tokens(&mut tokens);

        let output = tokens.to_string();
        // Consume-and-return: forget takes the entity by value and returns the
        // rebuilt (forgotten) entity — no `&mut`, no in-place assignment.
        assert!(output.contains("entity : Entity) -> Result < Entity , EntityForgetError >"));
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
        assert!(output.contains("Self :: extract_concurrent_modification"));
        assert!(output.contains("EntityForgetError :: ConcurrentModification"));
        // No framework-appended marker: erasure events are a client convention.
        assert!(!output.contains(":: Forgot"));
        assert!(output.contains("forget_and_take (EntityEvent :: forget_forgettable_payloads)"));
    }

    #[test]
    fn forget_fn_nulls_index_columns() {
        let id = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let event = Ident::new("EntityEvent", Span::call_site());
        let error = Ident::new("EntityForgetError", Span::call_site());
        let email = Ident::new("email", Span::call_site());

        let forget_fn = ForgetFn {
            id: &id,
            entity: &entity,
            event: &event,
            error,
            table_name: "entities",
            events_table_name: "entity_events",
            event_ctx: false,
            forgettable_table_name: "entities_forgettable_payloads",
            forgettable_columns: vec![&email],
            nested_forget_fn_names: Vec::new(),
            post_persist_error: None,
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
        let error = Ident::new("EntityForgetError", Span::call_site());
        let hook_error: syn::Type = syn::parse_str("MyHookError").unwrap();

        let forget_fn = ForgetFn {
            id: &id,
            entity: &entity,
            event: &event,
            error,
            table_name: "entities",
            events_table_name: "entity_events",
            event_ctx: false,
            forgettable_table_name: "entities_forgettable_payloads",
            forgettable_columns: Vec::new(),
            nested_forget_fn_names: Vec::new(),
            post_persist_error: Some(&hook_error),
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
        assert!(output.contains("EntityForgetError :: PostPersistHookError"));
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
        let error = Ident::new("EntityForgetError", Span::call_site());
        let email = Ident::new("email", Span::call_site());
        let hook_error: syn::Type = syn::parse_str("MyHookError").unwrap();

        let forget_fn = ForgetFn {
            id: &id,
            entity: &entity,
            event: &event,
            error,
            table_name: "entities",
            events_table_name: "entity_events",
            event_ctx: false,
            forgettable_table_name: "entities_forgettable_payloads",
            forgettable_columns: vec![&email],
            nested_forget_fn_names: Vec::new(),
            post_persist_error: Some(&hook_error),
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
        assert!(output.contains("EntityForgetError :: PostPersistHookError"));
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

    /// `forget` must descend into nested children the same way `delete` does
    /// — one generated call per nested field, scoped to the entity by id —
    /// rather than leaving a child's forgettable data untouched when its
    /// parent is forgotten.
    #[test]
    fn forget_fn_cascades_into_nested_children() {
        let id = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let event = Ident::new("EntityEvent", Span::call_site());
        let error = Ident::new("EntityForgetError", Span::call_site());
        let nested_fn_name = Ident::new("forget_nested_users_in_op", Span::call_site());

        let forget_fn = ForgetFn {
            id: &id,
            entity: &entity,
            event: &event,
            error,
            table_name: "entities",
            events_table_name: "entity_events",
            event_ctx: false,
            forgettable_table_name: "entities_forgettable_payloads",
            forgettable_columns: Vec::new(),
            nested_forget_fn_names: vec![nested_fn_name],
            post_persist_error: None,
        };

        let mut tokens = TokenStream::new();
        forget_fn.to_tokens(&mut tokens);

        let output = tokens.to_string();
        assert!(output.contains(
            "Self :: forget_nested_users_in_op :: < _ , _ , EntityForgetError > (op , & entity) . await ?"
        ));
        // Cascades before the entity's own payload row is touched, matching
        // `delete`'s cascade-then-own-row order.
        let cascade_at = output
            .find("forget_nested_users_in_op")
            .expect("cascade call present");
        let delete_at = output
            .find("DELETE FROM entities_forgettable_payloads WHERE entity_id = $1")
            .expect("own payload delete present");
        assert!(
            cascade_at < delete_at,
            "must cascade BEFORE own payload delete"
        );
    }
}
