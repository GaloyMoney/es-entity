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
        }
    }
}

impl ToTokens for ForgetFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let id_type = &self.id;
        let entity_type = self.entity;
        let event_type = self.event;
        let error = &self.error;

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
        let persist_and_forget_columns = if self.forgettable_columns.is_empty() {
            quote! {
                if entity.events().any_new() {
                    Self::extract_concurrent_modification(
                        self.persist_events(op, entity.events_mut()).await,
                        #error::ConcurrentModification,
                    )?;
                }
            }
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

            quote! {
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
            }
        };

        // Marking has to wait until the payload delete has had its turn with
        // `&entity.id`, since it needs the events mutably.
        let mark_persisted = if self.forgettable_columns.is_empty() {
            quote! {}
        } else {
            quote! {
                if has_new_events {
                    // No row means the CTE's UPDATE matched nothing — the
                    // entity was hard-deleted underneath us. Report the lost
                    // race rather than an internal error.
                    let recorded_at = rows
                        .first()
                        .map(|row| row.recorded_at)
                        .ok_or(#error::ConcurrentModification)?;
                    entity.events_mut().mark_new_events_persisted_at(recorded_at);
                }
            }
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
            pub async fn forget_in_op<OP>(
                &self,
                op: &mut OP,
                mut entity: #entity_type
            ) -> Result<#entity_type, #error>
            where
                OP: es_entity::AtomicOperation
            {
                #persist_and_forget_columns
                {
                    let id = &entity.id;
                    sqlx::query!(
                        #query,
                        id as &#id_type
                    )
                    .execute(op.as_executor())
                    .await?;
                }
                #mark_persisted
                let events = entity.events_mut().forget_and_take(
                    #event_type::forget_forgettable_payloads
                );
                Ok(es_entity::TryFromEvents::try_from_events(events)?)
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
        };

        let mut tokens = TokenStream::new();
        forget_fn.to_tokens(&mut tokens);

        let output = tokens.to_string();
        // Consume-and-return: forget takes the entity by value and returns the
        // rebuilt (forgotten) entity — no `&mut`, no in-place assignment.
        assert!(output.contains("entity : Entity) -> Result < Entity , EntityForgetError >"));
        assert!(!output.contains("& mut Entity"));
        assert!(!output.contains("* entity ="));
        assert!(output.contains("Ok (es_entity :: TryFromEvents :: try_from_events"));
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
}
