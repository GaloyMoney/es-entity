//! Shared pieces of the combined index+events write statements.
//!
//! Every write path (`create`, `create_all`, `update`, `update_all`, `delete`,
//! `forget`) emits the same tail: an `INSERT INTO {events_table} … SELECT …
//! FROM UNNEST(…) RETURNING recorded_at` fed by arrays of serialized events.
//! Only the *head* — the data-modifying CTE that writes the index row — is
//! genuinely per-path. This module owns the tail, the preamble that gathers
//! the arrays, and the follow-up forgettable payload insert, so the
//! `event_context` branching and the `$n` placeholder arithmetic each exist
//! once instead of six times.

use proc_macro2::TokenStream;
use quote::quote;

/// Where the event rows' ids and sequences come from.
///
/// Two orthogonal axes: whether one entity's events or many are being written
/// (which decides what the arrays carry), and whether the id comes from a
/// data-modifying CTE in the same statement or is supplied directly (the
/// standalone `persist_events` helpers, used where there is no index write to
/// fold into).
pub enum EventSource<'a> {
    /// One entity, id bound directly — the standalone `persist_events`.
    PerEntityStandalone {
        id_param: usize,
        offset_param: usize,
    },
    /// One entity, id taken from a CTE the same statement writes. The event
    /// arrays are crossed with the single CTE row and the sequence comes from
    /// `ROW_NUMBER()`. `offset_param` is `None` where the entity is brand new
    /// and sequences therefore start at 1 (`create`).
    PerEntityCte {
        cte: &'a str,
        offset_param: Option<usize>,
    },
    /// Many entities, ids and sequences in the arrays — the standalone
    /// `persist_events_batch`.
    BatchStandalone,
    /// Many entities, joined back to a CTE the same statement writes, which
    /// both orders the writes and reveals index rows that vanished (a short
    /// `RETURNING` count).
    BatchCte { cte: &'a str },
}

impl EventSource<'_> {
    fn is_batch(&self) -> bool {
        matches!(self, Self::BatchStandalone | Self::BatchCte { .. })
    }
}

/// Emitter for the events-table half of a combined write statement.
pub struct EventsInsert<'a> {
    pub events_table: &'a str,
    pub event_ctx: bool,
}

impl<'a> EventsInsert<'a> {
    pub fn new(events_table: &'a str, event_ctx: bool) -> Self {
        Self {
            events_table,
            event_ctx,
        }
    }

    /// The `INSERT INTO {events} … RETURNING recorded_at` tail.
    ///
    /// `now_param` is the placeholder holding `op.maybe_now()` (shared with
    /// the head, which uses it for `created_at`); `first_array_param` is the
    /// first placeholder this tail owns.
    pub fn sql(
        &self,
        source: &EventSource<'_>,
        now_param: usize,
        first_array_param: usize,
    ) -> String {
        let ctx_col = if self.event_ctx { ", context" } else { "" };
        let ctx_sel = if self.event_ctx {
            ", unnested.context"
        } else {
            ""
        };
        let p = first_array_param;

        let (id_expr, sequence_expr, from_clause) = match source {
            EventSource::PerEntityStandalone {
                id_param,
                offset_param,
            } => (
                format!("${id_param}"),
                format!("ROW_NUMBER() OVER () + ${offset_param}"),
                self.per_entity_unnest(p, None),
            ),
            EventSource::PerEntityCte { cte, offset_param } => {
                let sequence_expr = match offset_param {
                    Some(offset) => format!("ROW_NUMBER() OVER () + ${offset}"),
                    None => "ROW_NUMBER() OVER ()".to_string(),
                };
                (
                    format!("{cte}.id"),
                    sequence_expr,
                    self.per_entity_unnest(p, Some(cte)),
                )
            }
            EventSource::BatchStandalone => (
                "unnested.id".to_string(),
                "unnested.sequence".to_string(),
                self.batch_unnest(p, None),
            ),
            EventSource::BatchCte { cte } => (
                "unnested.id".to_string(),
                "unnested.sequence".to_string(),
                self.batch_unnest(p, Some(cte)),
            ),
        };

        format!(
            "INSERT INTO {} (id, recorded_at, sequence, event_type, event{ctx_col}) \
             SELECT {id_expr}, COALESCE(${now_param}, NOW()), {sequence_expr}, unnested.event_type, unnested.event{ctx_sel} \
             {from_clause} \
             RETURNING recorded_at",
            self.events_table,
        )
    }

    /// `FROM` clause for the one-entity shapes: the arrays carry only the
    /// event type and body, optionally crossed with a CTE row supplying the id.
    fn per_entity_unnest(&self, p: usize, cte: Option<&str>) -> String {
        let ctx_col = if self.event_ctx { ", context" } else { "" };
        let ctx_unnest = if self.event_ctx {
            format!(", ${}::JSONB[]", p + 2)
        } else {
            String::new()
        };
        let cross_join = match cte {
            Some(cte) => format!("{cte} CROSS JOIN "),
            None => String::new(),
        };
        format!(
            "FROM {cross_join}UNNEST(${}::TEXT[], ${}::JSONB[]{ctx_unnest}) AS unnested(event_type, event{ctx_col})",
            p,
            p + 1,
        )
    }

    /// `FROM` clause for the many-entity shapes: the arrays also carry ids and
    /// sequences, optionally joined back to a CTE.
    fn batch_unnest(&self, p: usize, cte: Option<&str>) -> String {
        let ctx_col = if self.event_ctx { ", context" } else { "" };
        let ctx_unnest = if self.event_ctx {
            format!(", ${}::JSONB[]", p + 4)
        } else {
            String::new()
        };
        let join = match cte {
            Some(cte) => format!(" JOIN {cte} ON {cte}.id = unnested.id"),
            None => String::new(),
        };
        format!(
            "FROM UNNEST(${}, ${}::INT[], ${}::TEXT[], ${}::JSONB[]{ctx_unnest}) AS unnested(id, sequence, event_type, event{ctx_col}){join}",
            p,
            p + 1,
            p + 2,
            p + 3,
        )
    }

    /// Statements that gather the serialized events for one entity into the
    /// locals the tail's arguments name. `events` is an expression evaluating
    /// to `&EntityEvents<_>`.
    pub fn gather_per_entity(&self, events: TokenStream) -> TokenStream {
        let ctx_var = if self.event_ctx {
            quote! { let contexts = #events.serialize_new_event_contexts(); }
        } else {
            quote! {}
        };
        quote! {
            let offset = #events.len_persisted();
            let events_types = #events.new_event_types();
            let serialized_events = #events.serialize_new_events();
            #ctx_var
        }
    }

    /// Declarations for the batch gather accumulators.
    pub fn batch_declarations(&self, id_type: &syn::Ident) -> TokenStream {
        let ctx_var = if self.event_ctx {
            quote! { let mut all_contexts: Vec<es_entity::ContextData> = Vec::new(); }
        } else {
            quote! {}
        };
        quote! {
            let mut all_ids: Vec<&#id_type> = Vec::new();
            let mut all_sequences: Vec<i32> = Vec::new();
            let mut all_types = Vec::new();
            let mut all_serialized = Vec::new();
            #ctx_var
        }
    }

    /// Per-entity body of the batch gather loop. `events` evaluates to
    /// `&EntityEvents<_>` and `id` to `&{IdType}`; both are re-read rather
    /// than threaded so callers keep their own borrow shapes.
    pub fn gather_batch(&self, events: TokenStream, id: TokenStream) -> TokenStream {
        let ctx_extend = if self.event_ctx {
            quote! {
                if let Some(contexts) = #events.serialize_new_event_contexts() {
                    all_contexts.extend(contexts);
                }
            }
        } else {
            quote! {}
        };
        quote! {
            let offset = #events.len_persisted() + 1;
            let types = #events.new_event_types();
            let serialized = #events.serialize_new_events();
            #ctx_extend

            let n_new = serialized.len();
            all_types.extend(types);
            all_serialized.extend(serialized);
            all_ids.extend(std::iter::repeat(#id).take(n_new));
            all_sequences.extend((offset..).take(n_new).map(|i| i as i32));
        }
    }

    /// The argument expressions the tail binds, in placeholder order, starting
    /// with `op.maybe_now()`. Callers wrap them for their own call style —
    /// positional `query!` arguments, `PgArguments::add`, or `.bind`.
    pub fn arg_exprs(&self, source: &EventSource<'_>) -> Vec<TokenStream> {
        let mut args = vec![quote! { op.maybe_now() }];
        if source.is_batch() {
            args.push(quote! { &all_ids });
            args.push(quote! { &all_sequences });
            args.push(quote! { &all_types });
            args.push(quote! { &all_serialized });
            if self.event_ctx {
                args.push(quote! {
                    &if all_contexts.is_empty() {
                        None
                    } else {
                        Some(all_contexts)
                    }
                });
            }
        } else {
            let has_offset = !matches!(
                source,
                EventSource::PerEntityCte {
                    offset_param: None,
                    ..
                }
            );
            if has_offset {
                args.push(quote! { offset as i32 });
            }
            args.push(quote! { &events_types });
            args.push(quote! { &serialized_events });
            if self.event_ctx {
                args.push(quote! { contexts.as_deref() as Option<&[es_entity::ContextData]> });
            }
        }
        args
    }
}

/// Emitter for the follow-up forgettable payload insert.
///
/// Payloads live in a third table, so they cannot join the combined statement:
/// sub-statements of a data-modifying CTE do not see each other's writes, and
/// `forget`'s payload delete has to be able to see them.
pub struct ForgettablePayloads<'a> {
    pub table: &'a str,
    pub id_type: &'a syn::Ident,
    pub event_type: &'a syn::Ident,
}

impl ForgettablePayloads<'_> {
    /// Collects and inserts the payloads of one entity's new events.
    /// Requires `offset` (the pre-persist `len_persisted`) and `id` in scope.
    pub fn insert_per_entity(&self, events: TokenStream, error: &syn::Ident) -> TokenStream {
        let Self {
            table,
            id_type,
            event_type,
        } = self;
        let query = format!(
            "INSERT INTO {table} (entity_id, sequence, payload) SELECT $1, unnested.sequence, unnested.payload FROM UNNEST($2::INT[], $3::JSONB[]) AS unnested(sequence, payload)"
        );
        quote! {
            let mut payload_sequences: Vec<i32> = Vec::new();
            let mut payload_values: Vec<es_entity::prelude::serde_json::Value> = Vec::new();
            for (idx, event_with_ctx) in #events.iter_new_events().enumerate() {
                if let Some(payload) = #event_type::extract_forgettable_payloads(&event_with_ctx.event) {
                    payload_sequences.push((offset + 1 + idx) as i32);
                    payload_values.push(payload);
                }
            }
            if !payload_sequences.is_empty() {
                Self::extract_concurrent_modification(
                    sqlx::query!(
                        #query,
                        id as &#id_type,
                        &payload_sequences,
                        &payload_values,
                    )
                    .execute(op.as_executor())
                    .await,
                    #error::ConcurrentModification,
                )?;
            }
        }
    }

    /// Declarations for the batch payload accumulators.
    pub fn batch_declarations(&self) -> TokenStream {
        let id_type = self.id_type;
        quote! {
            let mut payload_ids: Vec<&#id_type> = Vec::new();
            let mut payload_sequences: Vec<i32> = Vec::new();
            let mut payload_values: Vec<es_entity::prelude::serde_json::Value> = Vec::new();
        }
    }

    /// Per-entity body of the batch payload gather. Requires `offset` (the
    /// 1-based first sequence for this entity) in scope.
    pub fn gather_batch(&self, events: TokenStream, id: TokenStream) -> TokenStream {
        let event_type = self.event_type;
        quote! {
            for (idx, event_with_ctx) in #events.iter_new_events().enumerate() {
                if let Some(payload) = #event_type::extract_forgettable_payloads(&event_with_ctx.event) {
                    payload_ids.push(#id);
                    payload_sequences.push((offset + idx) as i32);
                    payload_values.push(payload);
                }
            }
        }
    }

    /// The batch payload insert.
    pub fn insert_batch(&self, error: &syn::Ident) -> TokenStream {
        let query = format!(
            "INSERT INTO {} (entity_id, sequence, payload) SELECT unnested.entity_id, unnested.sequence, unnested.payload FROM UNNEST($1, $2::INT[], $3::JSONB[]) AS unnested(entity_id, sequence, payload)",
            self.table
        );
        quote! {
            if !payload_sequences.is_empty() {
                Self::extract_concurrent_modification(
                    sqlx::query(#query)
                        .bind(&payload_ids)
                        .bind(&payload_sequences)
                        .bind(&payload_values)
                        .execute(op.as_executor())
                        .await,
                    #error::ConcurrentModification,
                )?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_entity_sql_without_offset_matches_create() {
        let insert = EventsInsert::new("entity_events", false);
        let source = EventSource::PerEntityCte {
            cte: "new_row",
            offset_param: None,
        };
        assert_eq!(
            insert.sql(&source, 2, 3),
            "INSERT INTO entity_events (id, recorded_at, sequence, event_type, event) \
             SELECT new_row.id, COALESCE($2, NOW()), ROW_NUMBER() OVER (), unnested.event_type, unnested.event \
             FROM new_row CROSS JOIN UNNEST($3::TEXT[], $4::JSONB[]) AS unnested(event_type, event) \
             RETURNING recorded_at"
        );
    }

    #[test]
    fn per_entity_sql_with_offset_and_context() {
        let insert = EventsInsert::new("entity_events", true);
        let source = EventSource::PerEntityCte {
            cte: "updated",
            offset_param: Some(4),
        };
        assert_eq!(
            insert.sql(&source, 3, 5),
            "INSERT INTO entity_events (id, recorded_at, sequence, event_type, event, context) \
             SELECT updated.id, COALESCE($3, NOW()), ROW_NUMBER() OVER () + $4, unnested.event_type, unnested.event, unnested.context \
             FROM updated CROSS JOIN UNNEST($5::TEXT[], $6::JSONB[], $7::JSONB[]) AS unnested(event_type, event, context) \
             RETURNING recorded_at"
        );
    }

    #[test]
    fn batch_sql_joins_the_cte() {
        let insert = EventsInsert::new("entity_events", false);
        let source = EventSource::BatchCte { cte: "new_rows" };
        assert_eq!(
            insert.sql(&source, 1, 4),
            "INSERT INTO entity_events (id, recorded_at, sequence, event_type, event) \
             SELECT unnested.id, COALESCE($1, NOW()), unnested.sequence, unnested.event_type, unnested.event \
             FROM UNNEST($4, $5::INT[], $6::TEXT[], $7::JSONB[]) AS unnested(id, sequence, event_type, event) \
             JOIN new_rows ON new_rows.id = unnested.id \
             RETURNING recorded_at"
        );
    }

    #[test]
    fn arg_exprs_follow_placeholder_order() {
        let insert = EventsInsert::new("entity_events", false);
        let per_entity = insert.arg_exprs(&EventSource::PerEntityCte {
            cte: "updated",
            offset_param: Some(3),
        });
        let rendered: Vec<_> = per_entity.iter().map(|t| t.to_string()).collect();
        assert_eq!(
            rendered,
            vec![
                "op . maybe_now ()",
                "offset as i32",
                "& events_types",
                "& serialized_events"
            ]
        );
    }
}
