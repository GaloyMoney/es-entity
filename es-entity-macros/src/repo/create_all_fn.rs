use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::{
    events_write::{EventSource, EventsInsert, ForgettablePayloads},
    options::*,
};

pub struct CreateAllFn<'a> {
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
    id_policy: IdPolicyOpt,
    /// Whether the repo's own batch statement can reject — see
    /// `RepositoryOptions::create_can_reject`. `false` narrows the
    /// statement's classification with `narrow_rejected()`.
    can_reject: bool,
    /// The generated signatures' error type, see
    /// `RepositoryOptions::create_error_ty`.
    write_error_ty: TokenStream,
    #[cfg(feature = "instrument")]
    repo_name_snake: String,
}

impl<'a> From<&'a RepositoryOptions> for CreateAllFn<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            id_policy: opts.id_policy(),
            can_reject: opts.create_can_reject(),
            write_error_ty: opts.create_error_ty(),
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

impl ToTokens for CreateAllFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let entity = self.entity;
        let constraint_violation = &self.constraint_violation;

        // Nested creation is batched once across the whole hydrated parent
        // batch (not once per parent), so it runs as its own pass after every
        // parent is hydrated, gathering `&mut` refs into every parent just
        // for that pass.
        let nested_phase = if self.nested_fn_names.is_empty() {
            quote! {}
        } else {
            let nested = self.nested_fn_names.iter().map(|f| {
                quote! {
                    self.#f(op, &mut entity_refs).await.map_err(es_entity::LiftNestedWrite::<#constraint_violation>::lift_nested)?;
                }
            });
            quote! {
                let mut entity_refs: Vec<&mut #entity> = entities.iter_mut().collect();
                #(#nested)*
                drop(entity_refs);
            }
        };

        let table_name = self.table_name;

        let column_names = self.columns.insert_column_names();
        let placeholders = self.columns.insert_placeholders(1);
        let (arg_collection, arg_adds) = self.columns.create_all_arg_collection(
            syn::parse_quote! { new_entity },
            self.id_policy.is_generated(),
        );

        // Both inserts in one statement. The events insert joins the `new_rows`
        // CTE, which reveals index rows that vanished (a short RETURNING
        // count). Postgres interleaves the CTE and main inserts with no
        // guaranteed ordering, so a duplicate id (including an intra-batch
        // one) may surface as either table's constraint — the classifier maps
        // both to the same `ConstraintViolation`. New entities are never
        // snapshotted here, even when the repo enables `snapshot` — each one
        // gets its first snapshot on its first `update`.
        let events_insert = EventsInsert::new(self.events_table_name, self.event_ctx);
        let source = EventSource::BatchCte { cte: "new_rows" };

        let query = format!(
            "WITH new_rows AS (INSERT INTO {} (created_at, {}) \
            SELECT COALESCE($1, NOW()), unnested.{} \
            FROM UNNEST({}) \
            AS unnested({}) RETURNING id) {}",
            table_name,
            column_names.join(", "),
            column_names.join(", unnested."),
            placeholders,
            column_names.join(", "),
            events_insert.sql(&source, 1, column_names.len() + 2),
        );

        let id_type = self.id;
        let generated = self.id_policy.is_generated();
        let resolve_ids = if generated {
            let resolved = self.id_policy.resolve(id_type);
            quote! {
                let mut __ids: Vec<#id_type> = Vec::with_capacity(new_entities.len());
                for _ in 0..new_entities.len() {
                    __ids.push(#resolved);
                }
            }
        } else {
            quote! {}
        };
        let (capture_supplied_ids, supplied_ids_check) = if generated {
            (quote! {}, quote! {})
        } else {
            (
                quote! {
                    let __supplied_ids: Vec<#id_type> = id_collection.iter().map(|id: &&#id_type| <#id_type as Clone>::clone(*id)).collect();
                },
                quote! {
                    if all_events.iter().zip(__supplied_ids.iter()).any(|(events, id): (&es_entity::EntityEvents<<#entity as es_entity::EsEntity>::Event, <#entity as es_entity::EsEntity>::Snapshot>, &#id_type)| events.id() != id) {
                        return Err(errlanes::Fatal::invariant("supplied id differs from the id the entity was built under").into());
                    }
                },
            )
        };
        let convert_all = if generated {
            let builder = self.id_policy.builder(quote! { id.clone() });
            quote! {
                new_entities
                    .into_iter()
                    .zip(__ids.iter())
                    .map(|(new_entity, id)| Self::convert_new(new_entity, #builder))
                    .collect()
            }
        } else {
            let builder = self.id_policy.builder(quote! {});
            quote! {
                new_entities
                    .into_iter()
                    .map(|new_entity| Self::convert_new(new_entity, #builder))
                    .collect()
            }
        };
        // Postgres reports the bare table name in errors, so a
        // schema-qualified events table is reduced to its last path
        // component before being compared against `DatabaseError::table()`.
        let events_table_bare = self
            .events_table_name
            .rsplit('.')
            .next()
            .unwrap_or(self.events_table_name);
        let classify_create = if generated {
            quote! { Self::classify_create_write(e) }
        } else {
            quote! {
                Self::classify_create_write(e, |db_err| {
                    // The database message is a lookup key, never a
                    // value: the attributed id is always one of the
                    // batch's own ids, found by matching the
                    // reported key text against them. A duplicate
                    // reported inside the batch itself still
                    // matches correctly -- both occurrences are the
                    // same id, so `find`'s first hit is right either
                    // way.
                    let reported = if db_err.table() == Some(#events_table_bare) {
                        es_entity::extract_events_pkey_id_value(db_err)
                    } else {
                        es_entity::extract_constraint_value(db_err)
                    }?;
                    all_ids.iter().find(|id| id.to_string() == reported).map(|id| (**id).clone())
                })
            }
        };
        let write_error_ty = &self.write_error_ty;
        let (narrow_import, narrow_call) = if self.can_reject {
            (quote! {}, quote! {})
        } else {
            (
                quote! { use es_entity::errlanes::ResultExt as _; },
                quote! { .narrow_rejected() },
            )
        };
        let batch_declarations = events_insert.batch_declarations(id_type);
        let gather = events_insert.gather_batch(quote! { events }, quote! { id });
        // The index columns are encoded first (their borrows on the new
        // entities must end before those are consumed), so the event arrays
        // follow — hence skipping the shared `now` argument here.
        let event_arg_adds = events_insert
            .arg_exprs(&source)
            .into_iter()
            .skip(1)
            .map(|expr| quote! { __query_args.add(#expr).map_err(sqlx::Error::Encode)?; });

        let payloads = self
            .forgettable_table_name
            .map(|table| ForgettablePayloads {
                table,
                id_type,
                event_type: self.event,
            });
        let forgettable_vars = payloads
            .as_ref()
            .map(|p| p.batch_declarations())
            .unwrap_or_default();
        let forgettable_extract = payloads
            .as_ref()
            .map(|p| p.gather_batch(quote! { events }, quote! { id }))
            .unwrap_or_default();
        let forgettable_insert = payloads
            .as_ref()
            .map(|p| p.insert_batch(self.events_table_name))
            .unwrap_or_default();

        #[cfg(feature = "instrument")]
        let (instrument_attr, error_recording) = {
            let entity_name = entity.to_string();
            let repo_name = &self.repo_name_snake;
            let span_name = format!("{}.create_all", repo_name);
            (
                quote! {
                    #[tracing::instrument(name = #span_name, skip_all, fields(entity = #entity_name, count = new_entities.len(), error = tracing::field::Empty, exception.message = tracing::field::Empty, exception.type = tracing::field::Empty))]
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
        let (instrument_attr, error_recording) = (quote! {}, quote! {});

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

        // The post-hydrate/post-persist pass only exists to run these two
        // hooks (nested creation is already batched by `nested_phase`
        // above), so it — and the `n_events` bookkeeping it needs — is
        // skipped entirely when neither hook is configured: an empty loop
        // would just be dead weight (and an unused-variable warning under
        // `-D warnings`).
        let post_checks_phase = if self.post_hydrate_hook || self.post_persist_hook {
            quote! {
                let mut n_events_by_entity: Vec<usize> = Vec::new();
                for (events, n_events) in all_events.into_iter().zip(n_persisted) {
                    let entity = Self::hydrate_entity(events)?;
                    entities.push(entity);
                    n_events_by_entity.push(n_events);
                }

                #nested_phase

                for (entity, n_events) in entities.iter().zip(n_events_by_entity) {
                    #post_hydrate_check
                    #post_persist_check
                }
            }
        } else {
            quote! {
                for (events, _n_events) in all_events.into_iter().zip(n_persisted) {
                    let entity = Self::hydrate_entity(events)?;
                    entities.push(entity);
                }

                #nested_phase
            }
        };

        let standalone = (!self.in_op_only).then(|| {
            quote! {
                pub async fn create_all(
                    &self,
                    new_entities: Vec<<#entity as es_entity::EsEntity>::New>
                ) -> Result<Vec<#entity>, #write_error_ty> {
                    let mut op = self.begin_op().await?;
                    let res = self.create_all_in_op(&mut op, new_entities).await?;
                    op.commit().await?;
                    Ok(res)
                }
            }
        });

        tokens.append_all(quote! {
            #standalone

            #instrument_attr
            pub async fn create_all_in_op<OP>(
                &self,
                op: &mut OP,
                new_entities: Vec<<#entity as es_entity::EsEntity>::New>
            ) -> Result<Vec<#entity>, #write_error_ty>
            where
                OP: es_entity::AtomicOperation + ?Sized
            {
                let __result: Result<Vec<#entity>, #write_error_ty> = async {
                    #narrow_import
                    use es_entity::prelude::sqlx::{Arguments, Row};

                    let mut entities = Vec::new();
                    if new_entities.is_empty() {
                        return Ok(entities);
                    }

                    #resolve_ids

                    let mut __query_args = sqlx::postgres::PgArguments::default();
                    __query_args.add(op.maybe_now()).map_err(sqlx::Error::Encode)?;

                    // The index columns are encoded first so that the borrows
                    // they hold on `new_entities` end here, freeing it to be
                    // consumed into the event stream below.
                    #arg_collection
                    #capture_supplied_ids
                    #(#arg_adds)*

                    let mut all_events: Vec<es_entity::EntityEvents<<#entity as es_entity::EsEntity>::Event, <#entity as es_entity::EsEntity>::Snapshot>> = #convert_all;
                    #supplied_ids_check

                    #batch_declarations
                    let mut n_persisted: Vec<usize> = Vec::new();
                    #forgettable_vars

                    for events in all_events.iter() {
                        let id = events.id();
                        #gather
                        #forgettable_extract
                        n_persisted.push(n_new);
                    }

                    #(#event_arg_adds)*

                    let expected_events = all_ids.len();
                    let rows = sqlx::query_with(#query, __query_args)
                        .fetch_all(op.as_executor())
                        .await
                        .map_err(|e| #classify_create)
                        #narrow_call?;

                    #forgettable_insert

                    if expected_events > 0 {
                        // Every event row joins an index row this same statement
                        // inserted, so a short count means the join dropped rows —
                        // a torn batch, not a race (every row belongs to this
                        // statement's own transaction).
                        if rows.len() != expected_events {
                            return Err(errlanes::Fatal::invariant(format!(
                                "batch wrote {} of {} events",
                                rows.len(),
                                expected_events
                            )).into());
                        }

                        let recorded_at = rows
                            .first()
                            .ok_or(sqlx::Error::RowNotFound)
                            .and_then(|row| row.try_get("recorded_at"))?;
                        for events in all_events.iter_mut() {
                            events.mark_new_events_persisted_at(recorded_at);
                        }
                    }

                    #post_checks_phase

                    Ok(entities)
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

    #[test]
    fn create_all_fn() {
        let entity = Ident::new("Entity", Span::call_site());
        let constraint_violation = syn::Ident::new("EntityConstraintViolation", Span::call_site());
        let id = Ident::new("EntityId", Span::call_site());
        let event = Ident::new("EntityEvent", Span::call_site());

        use darling::FromMeta;
        let input: syn::Meta = syn::parse_quote!(columns(name = "String",));
        let mut columns = Columns::from_meta(&input).expect("Failed to parse Fields");
        columns.set_id_column(&id);

        let create_fn = CreateAllFn {
            in_op_only: false,
            table_name: "entities",
            entity: &entity,
            id: &id,
            event: &event,
            events_table_name: "entity_events",
            event_ctx: false,
            forgettable_table_name: None,
            constraint_violation,
            columns: &columns,
            nested_fn_names: Vec::new(),
            post_hydrate_hook: false,
            post_persist_hook: false,
            id_policy: IdPolicyOpt::Supplied,
            can_reject: true,
            write_error_ty: quote! { es_entity::RepoWriteError<EntityConstraintViolation> },
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        create_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn create_all(
                &self,
                new_entities: Vec<<Entity as es_entity::EsEntity>::New>
            ) -> Result<Vec<Entity>, es_entity::RepoWriteError<EntityConstraintViolation> > {
                let mut op = self.begin_op().await?;
                let res = self.create_all_in_op(&mut op, new_entities).await?;
                op.commit().await?;
                Ok(res)
            }

            pub async fn create_all_in_op<OP>(
                &self,
                op: &mut OP,
                new_entities: Vec<<Entity as es_entity::EsEntity>::New>
            ) -> Result<Vec<Entity>, es_entity::RepoWriteError<EntityConstraintViolation> >
            where
                OP: es_entity::AtomicOperation + ?Sized
            {
                let __result: Result<Vec<Entity>, es_entity::RepoWriteError<EntityConstraintViolation> > = async {
                    use es_entity::prelude::sqlx::{Arguments, Row};

                    let mut entities = Vec::new();
                    if new_entities.is_empty() {
                        return Ok(entities);
                    }

                    let mut __query_args = sqlx::postgres::PgArguments::default();
                    __query_args.add(op.maybe_now()).map_err(sqlx::Error::Encode)?;

                    let mut id_collection = Vec::new();
                    let mut name_collection = Vec::new();

                    for new_entity in new_entities.iter() {
                        let id: &EntityId = &new_entity.id;
                        let name: &String = &new_entity.name;

                        id_collection.push(id);
                        name_collection.push(name);
                    }

                    let __supplied_ids: Vec<EntityId> = id_collection.iter().map(|id: &&EntityId| <EntityId as Clone>::clone(*id)).collect();
                    __query_args.add(id_collection).map_err(sqlx::Error::Encode)?;
                    __query_args.add(name_collection).map_err(sqlx::Error::Encode)?;

                    let mut all_events: Vec<es_entity::EntityEvents<<Entity as es_entity::EsEntity>::Event, <Entity as es_entity::EsEntity>::Snapshot>> = new_entities.into_iter().map(|new_entity| Self::convert_new(new_entity, es_entity::EntityEventsBuilder::__supplied())).collect();
                    if all_events.iter().zip(__supplied_ids.iter()).any(|(events, id): (&es_entity::EntityEvents<<Entity as es_entity::EsEntity>::Event, <Entity as es_entity::EsEntity>::Snapshot>, &EntityId)| events.id() != id) {
                        return Err(errlanes::Fatal::invariant("supplied id differs from the id the entity was built under").into());
                    }

                    let mut all_ids: Vec<&EntityId> = Vec::new();
                    let mut all_sequences: Vec<i32> = Vec::new();
                    let mut all_types = Vec::new();
                    let mut all_serialized = Vec::new();
                    let mut n_persisted: Vec<usize> = Vec::new();

                    for events in all_events.iter() {
                        let id = events.id();
                        let offset = events.len_persisted() + 1;
                        let types = events.new_event_types();
                        let serialized = events.serialize_new_events();

                        let n_new = serialized.len();
                        all_types.extend(types);
                        all_serialized.extend(serialized);
                        all_ids.extend(std::iter::repeat(id).take(n_new));
                        all_sequences.extend((offset..).take(n_new).map(|i| i as i32));
                        n_persisted.push(n_new);
                    }

                    __query_args.add(&all_ids).map_err(sqlx::Error::Encode)?;
                    __query_args.add(&all_sequences).map_err(sqlx::Error::Encode)?;
                    __query_args.add(&all_types).map_err(sqlx::Error::Encode)?;
                    __query_args.add(&all_serialized).map_err(sqlx::Error::Encode)?;

                    let expected_events = all_ids.len();
                    let rows = sqlx::query_with(
                        "WITH new_rows AS (INSERT INTO entities (created_at, id, name) SELECT COALESCE($1, NOW()), unnested.id, unnested.name FROM UNNEST($2, $3) AS unnested(id, name) RETURNING id) INSERT INTO entity_events (id, recorded_at, sequence, event_type, event) SELECT unnested.id, COALESCE($1, NOW()), unnested.sequence, unnested.event_type, unnested.event FROM UNNEST($4, $5::INT[], $6::TEXT[], $7::JSONB[]) AS unnested(id, sequence, event_type, event) JOIN new_rows ON new_rows.id = unnested.id RETURNING recorded_at",
                        __query_args
                    )
                        .fetch_all(op.as_executor())
                        .await
                        .map_err(|e| Self::classify_create_write(e, |db_err| {
                            let reported = if db_err.table() == Some("entity_events") {
                                es_entity::extract_events_pkey_id_value(db_err)
                            } else {
                                es_entity::extract_constraint_value(db_err)
                            }?;
                            all_ids.iter().find(|id| id.to_string() == reported).map(|id| (**id).clone())
                        }))?;

                    if expected_events > 0 {
                        if rows.len() != expected_events {
                            return Err(errlanes::Fatal::invariant(format!(
                                "batch wrote {} of {} events",
                                rows.len(),
                                expected_events
                            )).into());
                        }

                        let recorded_at = rows
                            .first()
                            .ok_or(sqlx::Error::RowNotFound)
                            .and_then(|row| row.try_get("recorded_at"))?;
                        for events in all_events.iter_mut() {
                            events.mark_new_events_persisted_at(recorded_at);
                        }
                    }

                    for (events, _n_events) in all_events.into_iter().zip(n_persisted) {
                        let entity = Self::hydrate_entity(events)?;
                        entities.push(entity);
                    }

                    Ok(entities)
                }.await;

                __result
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
