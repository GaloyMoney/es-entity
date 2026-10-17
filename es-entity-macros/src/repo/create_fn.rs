use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::{
    events_write::{EventSource, EventsInsert, ForgettablePayloads},
    options::*,
};

pub struct CreateFn<'a> {
    in_op_only: bool,
    entity: &'a syn::Ident,
    id: &'a syn::Ident,
    event: &'a syn::Ident,
    table_name: &'a str,
    events_table_name: &'a str,
    event_ctx: bool,
    forgettable_table_name: Option<&'a str>,
    columns: &'a Columns,
    constraint_violation: syn::Ident,
    nested_fn_names: Vec<syn::Ident>,
    post_hydrate_hook: bool,
    post_persist_hook: bool,
    id_source: IdSourceOpt,
    /// `RepositoryOptions::create_can_reject`: `false` narrows every
    /// generated signature from `RepoWriteError<CV>` to `RepoFault`.
    can_reject: bool,
    /// The generated signatures' error type, see
    /// `RepositoryOptions::create_error_ty`.
    write_error_ty: TokenStream,
    /// The disjuncts of the `create_or_find_in_op` lookup, see
    /// `RepositoryOptions::ensure_keys`.
    ensure_keys: Vec<EnsureKey>,
    /// `Some` when `create_or_find_in_op` can reject, see
    /// `RepositoryOptions::ensure_can_reject`.
    ensure_rejection: Option<syn::Ident>,
    /// `{Entity}Scope` on a scoped repo.
    scope_ty: Option<syn::Ident>,
    /// `delete = "soft"` or `"soft_without_queries"`.
    soft: bool,
    any_nested: bool,
    tbl_prefix: Option<&'a syn::LitStr>,
    snapshot_table_name: Option<&'a str>,
    /// The catalog names of the constraints behind `ensure_keys`.
    key_constraints: Vec<String>,
    #[cfg(feature = "instrument")]
    repo_name_snake: String,
}

impl<'a> From<&'a RepositoryOptions> for CreateFn<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            id_source: opts.id_source(),
            can_reject: opts.create_can_reject(),
            write_error_ty: opts.create_error_ty(),
            ensure_keys: opts.ensure_keys(),
            ensure_rejection: opts.ensure_can_reject().then(|| opts.ensure_rejection()),
            scope_ty: (!opts.columns.scope_columns().is_empty()).then(|| opts.scope_type_ident()),
            soft: opts.delete.is_soft(),
            any_nested: opts.any_nested(),
            tbl_prefix: opts.table_prefix(),
            snapshot_table_name: opts.snapshot_table_name(),
            key_constraints: opts.ensure_key_constraint_names(),
            in_op_only: opts.in_op_only(),
            table_name: opts.table_name(),
            entity: opts.entity(),
            id: opts.id(),
            event: opts.event(),
            events_table_name: opts.events_table_name(),
            event_ctx: opts.event_context_enabled(),
            forgettable_table_name: opts.forgettable_table_name(),
            constraint_violation: opts.constraint_violation(),
            nested_fn_names: opts
                .all_nested()
                .map(|f| f.create_nested_fn_name())
                .collect(),
            columns: &opts.columns,
            post_hydrate_hook: opts.post_hydrate_hook.is_some(),
            post_persist_hook: opts.post_persist_hook.is_some(),
            #[cfg(feature = "instrument")]
            repo_name_snake: opts.repo_name_snake_case(),
        }
    }
}

impl ToTokens for CreateFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let entity = self.entity;
        let constraint_values = if self.can_reject {
            self.columns.constraint_values(entity, true)
        } else {
            quote! {}
        };
        let constraint_violation = &self.constraint_violation;
        let write_error_ty = &self.write_error_ty;
        let (rejection_prelude, rejection_tail) = if self.can_reject {
            (
                quote! { use es_entity::errlanes::ResultExt as _; },
                quote! { .map_rejected(|r| r.with_attempted(__constraint_values))? },
            )
        } else {
            (
                quote! { use es_entity::errlanes::ResultExt as _; },
                quote! { .narrow_rejected()? },
            )
        };
        let generated = self.id_source.is_repo();
        let id_source = self.id_source.marker();
        let resolve_id = if generated {
            let id_ty = self.id;
            let resolved = self.id_source.resolve(id_ty);
            quote! { let __id: #id_ty = #resolved; }
        } else {
            quote! {}
        };
        let builder = self.id_source.builder(quote! { __id.clone() });
        let (attempted_id, caller_id_check, classify_create) = if generated {
            (
                quote! {},
                quote! {},
                quote! { Self::classify_create_write(e) },
            )
        } else {
            (
                quote! {
                    // A single create already knows its id from its own
                    // input, so a duplicate-id conflict always attributes
                    // to exactly this id -- captured before `new_entity` is
                    // consumed into `events` below.
                    let __attempted_id = (*id).clone();
                },
                quote! {
                    if events.id() != &__attempted_id {
                        return Err(errlanes::Fatal::invariant("caller's id differs from the id the entity was built under").into());
                    }
                },
                quote! { Self::classify_create_write(e, move |_| Some(__attempted_id)) },
            )
        };

        let nested = |target: &TokenStream| -> Vec<TokenStream> {
            self.nested_fn_names
                .iter()
                .map(|f| {
                    quote! {
                        self.#f(op, &mut [&mut entity]).await.map_err(es_entity::LiftNestedWrite::<#target>::lift_nested)?;
                    }
                })
                .collect()
        };
        let maybe_mut_entity = if self.nested_fn_names.is_empty() {
            quote! { entity }
        } else {
            quote! { mut entity }
        };
        let assignments = self
            .columns
            .variable_assignments_for_create(syn::parse_quote! { new_entity }, generated);

        let table_name = self.table_name;

        let column_names = self.columns.insert_column_names();
        let placeholders = self.columns.insert_placeholders(0);
        let arg_adds = self.columns.create_query_arg_adds();

        // Index insert and events insert combined into one statement. The
        // outer insert reads from the CTE, which provides the id without a
        // second bind. Postgres interleaves the CTE and main inserts with no
        // guaranteed ordering, so a duplicate id may surface as either
        // table's constraint — the classifier maps both to the same
        // `ConstraintViolation`.
        //
        // A brand-new entity has no persisted events, so sequences start at 1
        // and no offset parameter is needed. A new entity is never
        // snapshotted here, even when its repo enables `snapshot` — it gets
        // its first snapshot on its first `update`.
        let events_insert = EventsInsert::new(self.events_table_name, self.event_ctx);
        let now_p = column_names.len() + 1;
        let source = EventSource::PerEntityCte {
            cte: "new_row",
            offset_param: None,
        };

        let query = |on_conflict: &str| {
            format!(
                "WITH new_row AS (INSERT INTO {} ({}, created_at) VALUES ({}, COALESCE(${}, NOW())){} RETURNING id) {}",
                table_name,
                column_names.join(", "),
                placeholders,
                now_p,
                on_conflict,
                events_insert.sql(&source, now_p, now_p + 1),
            )
        };

        // The event arrays are encoded after the index columns, whose borrows
        // on the new entity must end before it is consumed into events.
        let event_arg_adds: Vec<_> = events_insert
            .arg_exprs(&source)
            .into_iter()
            .skip(1)
            .map(|expr| quote! { __query_args.add(#expr).map_err(sqlx::Error::Encode)?; })
            .collect();
        let ctx_var = if self.event_ctx {
            quote! { let contexts = events.serialize_new_event_contexts(); }
        } else {
            quote! {}
        };

        let forgettable_code = match self.forgettable_table_name {
            Some(table) => {
                let payloads = ForgettablePayloads {
                    table,
                    id_type: self.id,
                    event_type: self.event,
                }
                .insert_per_entity(quote! { events }, self.events_table_name, None);
                quote! {
                    let offset = events.len_persisted();
                    let id = events.id();
                    #payloads
                }
            }
            None => quote! {},
        };

        #[cfg(feature = "instrument")]
        let (instrument_attr, record_id, error_recording) = {
            let entity_name = entity.to_string();
            let repo_name = &self.repo_name_snake;
            let span_name = format!("{}.create", repo_name);
            (
                quote! {
                    #[tracing::instrument(name = #span_name, skip_all, fields(entity = #entity_name, id = tracing::field::Empty, error = tracing::field::Empty, exception.message = tracing::field::Empty, exception.type = tracing::field::Empty))]
                },
                quote! {
                    tracing::Span::current().record("id", tracing::field::debug(&id));
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

        let post_hydrate_check = if self.post_hydrate_hook {
            quote! {
                self.execute_post_hydrate_hook(&entity)?;
            }
        } else {
            quote! {}
        };

        let post_persist_check = if self.post_persist_hook {
            quote! {
                self.execute_post_persist_hook(op, &entity, entity.events().last_persisted(n_events)).await?;
            }
        } else {
            quote! {}
        };

        let standalone = (!self.in_op_only).then(|| {
            quote! {
                pub async fn create(
                    &self,
                    new_entity: <#entity as es_entity::EsEntity>::New
                ) -> Result<#entity, #write_error_ty> {
                    let mut op = self.begin_op().await?;
                    let res = self.create_in_op(&mut op, new_entity).await?;
                    op.commit().await?;
                    Ok(res)
                }
            }
        });

        let id_ty = self.id;
        let create_query = query("");
        let create_nested = nested(&quote! { #constraint_violation });
        let ensure_fn = (!self.ensure_keys.is_empty()).then(|| {
            let missing_context = format!(
                "{table_name}: ON CONFLICT DO NOTHING inserted no row and no unique key matches a stored row"
            );
            let ensure_classify = if generated {
                classify_create.clone()
            } else {
                quote! {
                    Self::classify_create_write(e, {
                        let __attempted = __attempted_id.clone();
                        move |_| Some(__attempted)
                    })
                }
            };
            let key_constraints = &self.key_constraints;
            let (error_ty, target, ensure_constraint_values, first_err, fallback_err) =
                match &self.ensure_rejection {
                    Some(rejection) => (
                        quote! { es_entity::RepoWriteError<#rejection> },
                        quote! { #rejection },
                        self.columns.constraint_values(entity, true),
                        quote! {{
                            let __fail = #ensure_classify;
                            #rejection::narrow_write(
                                __fail.map_rejected(|r| r.with_attempted(__constraint_values)),
                            )
                        }},
                        quote! {
                            #rejection::narrow_fallback(
                                #ensure_classify,
                                #missing_context,
                            )
                        },
                    ),
                    None => (
                        quote! { es_entity::RepoFault },
                        quote! { #constraint_violation },
                        quote! {},
                        quote! { #ensure_classify.narrow_rejected() },
                        quote! {
                            match #ensure_classify.rejected() {
                                Ok(__violation) => {
                                    let __name = __violation.constraint_name();
                                    let __keys: &[&str] = &[#(#key_constraints),*];
                                    if __keys.contains(&__name) {
                                        es_entity::errlanes::Transient::new(
                                            es_entity::errlanes::TransientKind::OptimisticConflict,
                                        )
                                        .with_context(#missing_context)
                                        .into()
                                    } else {
                                        es_entity::errlanes::Fatal::invariant(
                                            "an unexpected unique constraint conflicted",
                                        )
                                        .with_context(__name.to_owned())
                                        .into()
                                    }
                                }
                                Err(fault) => fault.into(),
                            }
                        },
                    ),
                };
            let ensure_query = query(" ON CONFLICT DO NOTHING");
            let mut next_param = 0;
            let conditions: Vec<String> = self
                .ensure_keys
                .iter()
                .map(|key| {
                    let mut parts: Vec<String> = key
                        .columns
                        .iter()
                        .map(|c| {
                            next_param += 1;
                            format!("{c} = ${next_param}")
                        })
                        .collect();
                    if key.live_only {
                        parts.push("deleted = FALSE".to_string());
                    }
                    if parts.len() == 1 {
                        parts.join("")
                    } else {
                        format!("({})", parts.join(" AND "))
                    }
                })
                .collect();
            let scope_columns = self.columns.scope_columns();
            let deleted_column = self
                .soft
                .then(|| "deleted IS DISTINCT FROM FALSE".to_string());
            let lookup_query = format!(
                "SELECT id, {} FROM {} WHERE {}",
                conditions
                    .iter()
                    .map(|c| format!("COALESCE({c}, FALSE)"))
                    .chain(deleted_column)
                    .chain(scope_columns.iter().map(|c| c.name().to_string()))
                    .collect::<Vec<_>>()
                    .join(", "),
                table_name,
                conditions.join(" OR ")
            );
            let spanning_context = format!("{table_name}: the unique keys match more than one row");
            let key_columns = self.ensure_keys.iter().flat_map(|k| k.columns.iter());
            let key_types = self.ensure_keys.iter().flat_map(|k| k.column_types.iter());
            let key_names: Vec<&str> = self.ensure_keys.iter().map(|k| k.name.as_str()).collect();
            let n_keys = self.ensure_keys.len();
            let ensure_nested = nested(&target);
            let mut docs = vec![
                "Creates the entity, or returns the stored one when the `new` entity's unique keys (a caller-chosen id included) already name it.",
                "",
                "- **`Created`**: the insert hit no unique constraint. Events are persisted and the post-persist hook runs, as in `create_in_op`.",
                "- **`Found`**: the insert hit a unique constraint, and the `new` entity's unique keys (a caller-chosen id included) match exactly one live stored entity. That entity is returned unchanged: `new`'s events are dropped, nothing is written, the hook does not run. The stored entity's id may differ from `new`'s id (it was found through another key). Callers take the id from the returned entity, never from the `new` they passed in. Live matches win: deleted matches are ignored when a live entity holds a key.",
            ];
            if self.soft {
                docs.push("- **`Rejected(KeysHeldByDeleted)`**: no live entity holds any of `new`'s keys, but one or more soft-deleted entities do. Nothing is written and the transaction stays usable. The rejection carries the ids of the deleted holders and the key names, never values.");
            }
            docs.extend([
                "- **`Rejected(KeysSpanEntities)`**: the `new` entity's keys match more than one stored entity (one holds its code, another its email). No single stored entity is the one the caller described. On a soft-delete repo only live entities count. This rejection does not abort the caller's transaction: the insert was `DO NOTHING` and the lookup is a `SELECT`.",
                "- **`Rejected(<constraint member>)`**: the insert conflicted on a unique arbiter that is not a lookup key (a partial or expression unique index, a unique on a column the create does not write as-is). It is the member `create_in_op` would reject with. The conflicting insert runs in a savepoint, so the caller's transaction stays usable.",
                "- **`Transient(OptimisticConflict)`**: the insert conflicted on a lookup key the lookup could not see. Not reachable under READ COMMITTED (the lookup runs as a second statement and sees the committed row); kept for REPEATABLE READ / SERIALIZABLE, where the insert itself usually raises 40001 first (`Transient(SerializationFailure)`).",
                "",
                "`Found` compares nothing but keys: a `new` entity whose non-key fields differ from the stored entity still gets the stored entity. Ensure never updates.",
            ]);
            let docs: Vec<String> = docs.into_iter().map(|line| format!(" {line}")).collect();
            let on_span = |ids: TokenStream, matched: TokenStream| match &self.ensure_rejection {
                Some(rejection) => quote! {
                    let __keys: Vec<&'static str> = [#(#key_names),*]
                        .into_iter()
                        .zip(#matched)
                        .filter(|(_, matched)| *matched)
                        .map(|(key, _)| key)
                        .collect();
                    return Err(es_entity::errlanes::Fail::Rejected(
                        #rejection::KeysSpanEntities { ids: #ids, keys: __keys },
                    ));
                },
                None => quote! {
                    return Err(es_entity::errlanes::Fatal::invariant(
                        "the unique keys match more than one row",
                    )
                    .with_context(#spanning_context)
                    .into());
                },
            };
            let track = |matched: TokenStream| {
                if self.ensure_rejection.is_some() {
                    quote! {
                        for (i, matched) in #matched.iter_mut().enumerate() {
                            *matched |= row.try_get::<bool, _>(i + 1)?;
                        }
                    }
                } else {
                    quote! {}
                }
            };
            let (matched_decl, deleted_decl) = if self.ensure_rejection.is_some() {
                (
                    quote! { let mut __matched = [false; #n_keys]; },
                    quote! { let mut __deleted_matched = [false; #n_keys]; },
                )
            } else {
                (quote! {}, quote! {})
            };
            let track_matches = track(quote! { __matched });
            let push_live = quote! {
                __found_ids.push(row.try_get(0)?);
                #track_matches
            };
            let (deleted_decl, push_found) = if self.soft {
                let track_deleted = track(quote! { __deleted_matched });
                let deleted_idx = n_keys + 1;
                (
                    quote! {
                        let mut __deleted_ids: Vec<#id_ty> = Vec::new();
                        #deleted_decl
                    },
                    quote! {
                        if row.try_get::<bool, _>(#deleted_idx)? {
                            __deleted_ids.push(row.try_get(0)?);
                            #track_deleted
                        } else {
                            #push_live
                        }
                    },
                )
            } else {
                (quote! {}, push_live)
            };
            let scope_offset = n_keys + 1 + usize::from(self.soft);
            let on_span_live = on_span(quote! { __found_ids }, quote! { __matched });
            let held_by_deleted = match (&self.ensure_rejection, self.soft) {
                (Some(rejection), true) => {
                    let keys = quote! {
                        [#(#key_names),*]
                            .into_iter()
                            .zip(__deleted_matched)
                            .filter(|(_, matched)| *matched)
                            .map(|(key, _)| key)
                            .collect::<Vec<&'static str>>()
                    };
                    quote! {
                        if !__deleted_ids.is_empty() {
                            return Err(es_entity::errlanes::Fail::Rejected(
                                #rejection::KeysHeldByDeleted {
                                    ids: __deleted_ids,
                                    keys: #keys,
                                },
                            ));
                        }
                    }
                }
                _ => quote! {},
            };
            let (scope_capture, scope_decl, collect_row, on_outside, find_scope) =
                match (&self.scope_ty, &self.ensure_rejection) {
                    (Some(scope_ty), Some(rejection)) => {
                        let names: Vec<_> = scope_columns.iter().map(|c| c.name()).collect();
                        let tys: Vec<_> = scope_columns.iter().map(|c| c.ty()).collect();
                        let vars: Vec<_> = names
                            .iter()
                            .map(|n| quote::format_ident!("__scope_{}", n))
                            .collect();
                        let idxs = (0..scope_columns.len()).map(|i| i + scope_offset);
                        (
                            quote! { #(let #vars: #tys = <#tys as Clone>::clone(#names);)* },
                            quote! { let mut __outside = [false; #n_keys]; },
                            quote! {
                                if #(row.try_get::<#tys, _>(#idxs)? == #vars)&&* {
                                    #push_found
                                } else {
                                    for (i, outside) in __outside.iter_mut().enumerate() {
                                        *outside |= row.try_get::<bool, _>(i + 1)?;
                                    }
                                }
                            },
                            quote! {
                                if __outside.iter().any(|outside| *outside) {
                                    let __keys: Vec<&'static str> = [#(#key_names),*]
                                        .into_iter()
                                        .zip(__outside)
                                        .filter(|(_, outside)| *outside)
                                        .map(|(key, _)| key)
                                        .collect();
                                    return Err(es_entity::errlanes::Fail::Rejected(
                                        #rejection::KeysHeldOutsideScope { keys: __keys },
                                    ));
                                }
                            },
                            quote! { #scope_ty::All, },
                        )
                    }
                    _ => (quote! {}, quote! {}, push_found, quote! {}, quote! {}),
                };
            quote! {
                #(#[doc = #docs])*
                pub async fn create_or_find_in_op<OP>(
                    &self,
                    op: &mut OP,
                    new_entity: <#entity as es_entity::EsEntity>::New
                ) -> Result<es_entity::Ensured<#entity>, #error_ty>
                where
                    OP: es_entity::AtomicOperation + ?Sized
                {
                    use es_entity::prelude::sqlx::{Arguments, Row};

                    #resolve_id
                    #assignments
                    #attempted_id
                    #ensure_constraint_values

                    let mut __query_args = sqlx::postgres::PgArguments::default();
                    #(#arg_adds)*
                    __query_args.add(op.maybe_now()).map_err(sqlx::Error::Encode)?;

                    let mut __key_args = sqlx::postgres::PgArguments::default();
                    #(__key_args.add(#key_columns as &#key_types).map_err(sqlx::Error::Encode)?;)*
                    #scope_capture

                    let mut events = Self::convert_new(new_entity, #builder);
                    #caller_id_check
                    let events_types = events.new_event_types();
                    let serialized_events = events.serialize_new_events();
                    #ctx_var
                    #(#event_arg_adds)*

                    let rows = match sqlx::query_with(#ensure_query, __query_args.clone())
                        .fetch_all(op.as_executor())
                        .await
                    {
                        Ok(rows) => rows,
                        Err(e) => return Err(#first_err.into()),
                    };

                    let rows = if rows.is_empty() {
                        let __lookup = sqlx::query_with(#lookup_query, __key_args)
                            .fetch_all(op.as_executor())
                            .await?;
                        let mut __found_ids: Vec<#id_ty> = Vec::new();
                        #matched_decl
                        #deleted_decl
                        #scope_decl
                        for row in &__lookup {
                            #collect_row
                        }
                        #on_outside
                        match __found_ids.len() {
                            0 => {
                                #held_by_deleted
                                let __fallback = sqlx::query_with(#create_query, __query_args);
                                let mut __savepoint =
                                    es_entity::SavepointOperation::begin_savepoint(&mut *op).await?;
                                match __fallback
                                    .fetch_all(es_entity::AtomicOperation::as_executor(&mut __savepoint))
                                    .await
                                {
                                    Ok(rows) => {
                                        __savepoint.release().await?;
                                        rows
                                    }
                                    Err(e) => {
                                        __savepoint.rollback().await?;
                                        return Err(#fallback_err);
                                    }
                                }
                            }
                            1 => {
                                let entity = self
                                    .find_by_id_in_op(&mut *op, #find_scope __found_ids.remove(0))
                                    .await?;
                                return Ok(es_entity::Ensured::Found(entity));
                            }
                            _ => {
                                #on_span_live
                            }
                        }
                    } else {
                        rows
                    };

                    #forgettable_code

                    let recorded_at = rows
                        .first()
                        .ok_or(sqlx::Error::RowNotFound)
                        .and_then(|row| row.try_get("recorded_at"))?;
                    let n_events = events.mark_new_events_persisted_at(recorded_at);
                    let #maybe_mut_entity = Self::hydrate_entity(events)?;

                    #(#ensure_nested)*

                    #post_hydrate_check
                    #post_persist_check
                    Ok(es_entity::Ensured::Created(entity))
                }
            }
        });

        tokens.append_all(quote! {
            #[inline(always)]
            fn convert_new<Entity, Event, Snapshot>(
                item: Entity,
                builder: es_entity::EntityEventsBuilder<Event, #id_source>,
            ) -> es_entity::EntityEvents<Event, Snapshot>
            where
                Entity: es_entity::IntoEvents<Event, IdSource = #id_source>,
                Event: es_entity::EsEvent,
                Snapshot: es_entity::EsSnapshot,
            {
                item.into_events(builder).widen_snapshot()
            }

            #[inline(always)]
            fn hydrate_entity<Entity>(events: es_entity::EntityEvents<<Entity as es_entity::EsEntity>::Event, <Entity as es_entity::EsEntity>::Snapshot>) -> Result<Entity, es_entity::EntityHydrationError>
            where
                Entity: es_entity::EsEntity,
            {
                es_entity::hydrate_checked(events)
            }

            #standalone

            #instrument_attr
            pub async fn create_in_op<OP>(
                &self,
                op: &mut OP,
                new_entity: <#entity as es_entity::EsEntity>::New
            ) -> Result<#entity, #write_error_ty>
            where
                OP: es_entity::AtomicOperation + ?Sized
            {
                let __result: Result<#entity, #write_error_ty> = async {
                    #rejection_prelude
                    use es_entity::prelude::sqlx::{Arguments, Row};

                    #resolve_id
                    #assignments
                    #attempted_id
                    #constraint_values
                    #record_id

                    let mut __query_args = sqlx::postgres::PgArguments::default();
                    #(#arg_adds)*
                    __query_args.add(op.maybe_now()).map_err(sqlx::Error::Encode)?;

                    let mut events = Self::convert_new(new_entity, #builder);
                    #caller_id_check
                    let events_types = events.new_event_types();
                    let serialized_events = events.serialize_new_events();
                    #ctx_var
                    #(#event_arg_adds)*

                    let rows = sqlx::query_with(#create_query, __query_args)
                        .fetch_all(op.as_executor())
                        .await
                        .map_err(|e| #classify_create)
                        #rejection_tail;

                    #forgettable_code

                    let recorded_at = rows
                        .first()
                        .ok_or(sqlx::Error::RowNotFound)
                        .and_then(|row| row.try_get("recorded_at"))?;
                    let n_events = events.mark_new_events_persisted_at(recorded_at);
                    let #maybe_mut_entity = Self::hydrate_entity(events)?;

                    #(#create_nested)*

                    #post_hydrate_check
                    #post_persist_check
                    Ok(entity)
                }.await;

                #error_recording
                __result
            }

            #ensure_fn
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proc_macro2::Span;
    use syn::Ident;

    #[test]
    fn create_fn() {
        let entity = Ident::new("Entity", Span::call_site());
        let constraint_violation = syn::Ident::new("EntityConstraintViolation", Span::call_site());
        let id = Ident::new("EntityId", Span::call_site());
        let event = Ident::new("EntityEvent", Span::call_site());
        let mut columns = Columns::default();
        columns.set_id_column(&id);

        let create_fn = CreateFn {
            in_op_only: false,
            table_name: "entities",
            entity: &entity,
            id: &id,
            event: &event,
            events_table_name: "entity_events",
            event_ctx: false,
            forgettable_table_name: None,
            constraint_violation: constraint_violation.clone(),
            columns: &columns,
            nested_fn_names: Vec::new(),
            post_hydrate_hook: false,
            post_persist_hook: false,
            id_source: IdSourceOpt::Caller,
            can_reject: true,
            write_error_ty: quote! { es_entity::RepoWriteError<EntityConstraintViolation> },
            ensure_keys: Vec::new(),
            ensure_rejection: None,
            scope_ty: None,
            soft: false,
            any_nested: false,
            tbl_prefix: None,
            snapshot_table_name: None,
            key_constraints: Vec::new(),
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        create_fn.to_tokens(&mut tokens);

        let expected = quote! {
            #[inline(always)]
            fn convert_new<Entity, Event, Snapshot>(
                item: Entity,
                builder: es_entity::EntityEventsBuilder<Event, es_entity::FromCaller>,
            ) -> es_entity::EntityEvents<Event, Snapshot>
            where
                Entity: es_entity::IntoEvents<Event, IdSource = es_entity::FromCaller>,
                Event: es_entity::EsEvent,
                Snapshot: es_entity::EsSnapshot,
            {
                item.into_events(builder).widen_snapshot()
            }

            #[inline(always)]
            fn hydrate_entity<Entity>(events: es_entity::EntityEvents<<Entity as es_entity::EsEntity>::Event, <Entity as es_entity::EsEntity>::Snapshot>) -> Result<Entity, es_entity::EntityHydrationError>
            where
                Entity: es_entity::EsEntity,
            {
                es_entity::hydrate_checked(events)
            }

            pub async fn create(
                &self,
                new_entity: <Entity as es_entity::EsEntity>::New
            ) -> Result<Entity, es_entity::RepoWriteError<EntityConstraintViolation> > {
                let mut op = self.begin_op().await?;
                let res = self.create_in_op(&mut op, new_entity).await?;
                op.commit().await?;
                Ok(res)
            }

            pub async fn create_in_op<OP>(
                &self,
                op: &mut OP,
                new_entity: <Entity as es_entity::EsEntity>::New
            ) -> Result<Entity, es_entity::RepoWriteError<EntityConstraintViolation> >
            where
                OP: es_entity::AtomicOperation + ?Sized
            {
                let __result: Result<Entity, es_entity::RepoWriteError<EntityConstraintViolation> > = async {
                    use es_entity::errlanes::ResultExt as _;
                    use es_entity::prelude::sqlx::{Arguments, Row};

                    let id = &new_entity.id;
                    let __attempted_id = (*id).clone();
                    let __constraint_values = EntityConstraintValues { id: Some((*id).clone()), };

                    let mut __query_args = sqlx::postgres::PgArguments::default();
                    __query_args.add(id as &EntityId).map_err(sqlx::Error::Encode)?;
                    __query_args.add(op.maybe_now()).map_err(sqlx::Error::Encode)?;

                    let mut events = Self::convert_new(new_entity, es_entity::EntityEventsBuilder::__from_caller());
                    if events.id() != &__attempted_id {
                        return Err(errlanes::Fatal::invariant("caller's id differs from the id the entity was built under").into());
                    }
                    let events_types = events.new_event_types();
                    let serialized_events = events.serialize_new_events();
                    __query_args.add(&events_types).map_err(sqlx::Error::Encode)?;
                    __query_args.add(&serialized_events).map_err(sqlx::Error::Encode)?;

                    let rows = sqlx::query_with(
                        "WITH new_row AS (INSERT INTO entities (id, created_at) VALUES ($1, COALESCE($2, NOW())) RETURNING id) INSERT INTO entity_events (id, recorded_at, sequence, event_type, event) SELECT new_row.id, COALESCE($2, NOW()), ROW_NUMBER() OVER (), unnested.event_type, unnested.event FROM new_row CROSS JOIN UNNEST($3::TEXT[], $4::JSONB[]) AS unnested(event_type, event) RETURNING recorded_at",
                        __query_args
                    )
                        .fetch_all(op.as_executor())
                        .await
                        .map_err(|e| Self::classify_create_write(e, move |_| Some(__attempted_id)))
                        .map_rejected(|r| r.with_attempted(__constraint_values))?;

                    let recorded_at = rows
                        .first()
                        .ok_or(sqlx::Error::RowNotFound)
                        .and_then(|row| row.try_get("recorded_at"))?;
                    let n_events = events.mark_new_events_persisted_at(recorded_at);
                    let entity = Self::hydrate_entity(events)?;

                    Ok(entity)
                }.await;

                __result
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn create_fn_generated_id_has_no_route_for_a_caller_id() {
        let entity = Ident::new("Entity", Span::call_site());
        let id = Ident::new("EntityId", Span::call_site());
        let event = Ident::new("EntityEvent", Span::call_site());
        let mut columns = Columns::default();
        columns.set_id_column(&id);

        let create_fn = CreateFn {
            in_op_only: false,
            table_name: "entities",
            entity: &entity,
            id: &id,
            event: &event,
            events_table_name: "entity_events",
            event_ctx: false,
            forgettable_table_name: None,
            constraint_violation: syn::Ident::new("EntityConstraintViolation", Span::call_site()),
            columns: &columns,
            nested_fn_names: Vec::new(),
            post_hydrate_hook: false,
            post_persist_hook: false,
            id_source: IdSourceOpt::default(),
            can_reject: false,
            write_error_ty: quote! { es_entity::RepoFault },
            ensure_keys: Vec::new(),
            ensure_rejection: None,
            scope_ty: None,
            soft: false,
            any_nested: false,
            tbl_prefix: None,
            snapshot_table_name: None,
            key_constraints: Vec::new(),
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        create_fn.to_tokens(&mut tokens);
        let out = tokens.to_string();

        assert!(
            out.contains("let __id : EntityId = EntityId :: new ()"),
            "{out}"
        );
        assert!(out.contains("let id = & __id"), "{out}");
        assert!(
            !out.contains("new_entity . id"),
            "a generated id must never be read off the New* type: {out}"
        );
        assert!(
            out.contains("EntityEventsBuilder :: __from_repo (__id . clone ())"),
            "{out}"
        );
        assert!(
            out.contains("IntoEvents < Event , IdSource = es_entity :: FromRepo >"),
            "{out}"
        );
        assert!(
            out.contains("Result < Entity , es_entity :: RepoFault >"),
            "a repo that cannot reject returns RepoFault: {out}"
        );
        assert!(!out.contains("RepoWriteError"), "{out}");
        assert!(!out.contains("map_rejected"), "{out}");
    }

    #[test]
    fn create_fn_with_columns() {
        let entity = Ident::new("Entity", Span::call_site());
        let constraint_violation = syn::Ident::new("EntityConstraintViolation", Span::call_site());
        let id = Ident::new("EntityId", Span::call_site());
        let event = Ident::new("EntityEvent", Span::call_site());

        use darling::FromMeta;
        let input: syn::Meta =
            syn::parse_quote!(columns(name(ty = "String", create(accessor = "name()"))));
        let mut columns = Columns::from_meta(&input).expect("Failed to parse Fields");
        columns.set_id_column(&id);

        let create_fn = CreateFn {
            in_op_only: false,
            table_name: "entities",
            entity: &entity,
            id: &id,
            event: &event,
            events_table_name: "entity_events",
            event_ctx: false,
            forgettable_table_name: None,
            constraint_violation: constraint_violation.clone(),
            columns: &columns,
            nested_fn_names: Vec::new(),
            post_hydrate_hook: false,
            post_persist_hook: false,
            id_source: IdSourceOpt::Caller,
            can_reject: true,
            write_error_ty: quote! { es_entity::RepoWriteError<EntityConstraintViolation> },
            ensure_keys: Vec::new(),
            ensure_rejection: None,
            scope_ty: None,
            soft: false,
            any_nested: false,
            tbl_prefix: None,
            snapshot_table_name: None,
            key_constraints: Vec::new(),
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        create_fn.to_tokens(&mut tokens);

        let expected = quote! {
            #[inline(always)]
            fn convert_new<Entity, Event, Snapshot>(
                item: Entity,
                builder: es_entity::EntityEventsBuilder<Event, es_entity::FromCaller>,
            ) -> es_entity::EntityEvents<Event, Snapshot>
            where
                Entity: es_entity::IntoEvents<Event, IdSource = es_entity::FromCaller>,
                Event: es_entity::EsEvent,
                Snapshot: es_entity::EsSnapshot,
            {
                item.into_events(builder).widen_snapshot()
            }

            #[inline(always)]
            fn hydrate_entity<Entity>(events: es_entity::EntityEvents<<Entity as es_entity::EsEntity>::Event, <Entity as es_entity::EsEntity>::Snapshot>) -> Result<Entity, es_entity::EntityHydrationError>
            where
                Entity: es_entity::EsEntity,
            {
                es_entity::hydrate_checked(events)
            }

            pub async fn create(
                &self,
                new_entity: <Entity as es_entity::EsEntity>::New
            ) -> Result<Entity, es_entity::RepoWriteError<EntityConstraintViolation> > {
                let mut op = self.begin_op().await?;
                let res = self.create_in_op(&mut op, new_entity).await?;
                op.commit().await?;
                Ok(res)
            }

            pub async fn create_in_op<OP>(
                &self,
                op: &mut OP,
                new_entity: <Entity as es_entity::EsEntity>::New
            ) -> Result<Entity, es_entity::RepoWriteError<EntityConstraintViolation> >
            where
                OP: es_entity::AtomicOperation + ?Sized
            {
                let __result: Result<Entity, es_entity::RepoWriteError<EntityConstraintViolation> > = async {
                    use es_entity::errlanes::ResultExt as _;
                    use es_entity::prelude::sqlx::{Arguments, Row};

                    let id = &new_entity.id;
                    let name = &new_entity.name();
                    let __attempted_id = (*id).clone();
                    let __constraint_values = EntityConstraintValues { id: Some((*id).clone()), name: Some((*name).clone()), };

                    let mut __query_args = sqlx::postgres::PgArguments::default();
                    __query_args.add(id as &EntityId).map_err(sqlx::Error::Encode)?;
                    __query_args.add(name as &String).map_err(sqlx::Error::Encode)?;
                    __query_args.add(op.maybe_now()).map_err(sqlx::Error::Encode)?;

                    let mut events = Self::convert_new(new_entity, es_entity::EntityEventsBuilder::__from_caller());
                    if events.id() != &__attempted_id {
                        return Err(errlanes::Fatal::invariant("caller's id differs from the id the entity was built under").into());
                    }
                    let events_types = events.new_event_types();
                    let serialized_events = events.serialize_new_events();
                    __query_args.add(&events_types).map_err(sqlx::Error::Encode)?;
                    __query_args.add(&serialized_events).map_err(sqlx::Error::Encode)?;

                    let rows = sqlx::query_with(
                        "WITH new_row AS (INSERT INTO entities (id, name, created_at) VALUES ($1, $2, COALESCE($3, NOW())) RETURNING id) INSERT INTO entity_events (id, recorded_at, sequence, event_type, event) SELECT new_row.id, COALESCE($3, NOW()), ROW_NUMBER() OVER (), unnested.event_type, unnested.event FROM new_row CROSS JOIN UNNEST($4::TEXT[], $5::JSONB[]) AS unnested(event_type, event) RETURNING recorded_at",
                        __query_args
                    )
                        .fetch_all(op.as_executor())
                        .await
                        .map_err(|e| Self::classify_create_write(e, move |_| Some(__attempted_id)))
                        .map_rejected(|r| r.with_attempted(__constraint_values))?;

                    let recorded_at = rows
                        .first()
                        .ok_or(sqlx::Error::RowNotFound)
                        .and_then(|row| row.try_get("recorded_at"))?;
                    let n_events = events.mark_new_events_persisted_at(recorded_at);
                    let entity = Self::hydrate_entity(events)?;

                    Ok(entity)
                }.await;

                __result
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
