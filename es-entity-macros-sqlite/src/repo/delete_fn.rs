use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::*;

pub struct DeleteFn<'a> {
    in_op_only: bool,
    id: &'a syn::Ident,
    entity: &'a syn::Ident,
    table_name: &'a str,
    events_table_name: &'a str,
    /// `snapshot` repo: `persist_events` takes a snapshot argument.
    snapshot: bool,
    columns: &'a Columns,
    delete_option: &'a DeleteOption,
    nested_delete_fn_names: Vec<syn::Ident>,
    post_persist_hook: bool,
    forgettable_table_name: Option<&'a str>,
    /// `RepositoryOptions::update_can_reject`: `false` narrows the generated
    /// signatures from `RepoWriteError<CV>` to `es_entity::RepoFault`.
    can_reject: bool,
    /// The generated signatures' error type, see
    /// `RepositoryOptions::update_error_ty`.
    write_error_ty: TokenStream,
    #[cfg(feature = "instrument")]
    repo_name_snake: String,
}

impl<'a> DeleteFn<'a> {
    pub fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            in_op_only: opts.in_op_only(),
            id: opts.id(),
            entity: opts.entity(),
            columns: &opts.columns,
            table_name: opts.table_name(),
            events_table_name: opts.events_table_name(),
            snapshot: opts.snapshot_enabled(),
            delete_option: &opts.delete,
            nested_delete_fn_names: opts
                .all_nested()
                .map(|f| f.delete_nested_fn_name())
                .collect(),
            post_persist_hook: opts.post_persist_hook.is_some(),
            forgettable_table_name: opts.forgettable_table_name(),
            can_reject: opts.update_can_reject(),
            write_error_ty: opts.update_error_ty(),
            #[cfg(feature = "instrument")]
            repo_name_snake: opts.repo_name_snake_case(),
        }
    }
}

impl ToTokens for DeleteFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        if !self.delete_option.is_soft() {
            return;
        }

        let entity = self.entity;
        let table_name = self.table_name;

        let nested_deletes = self.nested_delete_fn_names.iter().map(|f| {
            quote! {
                Self::#f(op, &entity).await?;
            }
        });

        // Soft-delete auto-forgets: forgettable index columns are set to NULL
        // (not re-persisted from the live entity), matching `forget()`.
        let assignments = self
            .columns
            .variable_assignments_for_delete(syn::parse_quote! { entity });
        let constraint_values = self.columns.delete_constraint_values(entity);
        let column_updates = self.columns.sql_updates_for_delete();
        let args = self.columns.update_query_args_for_delete();

        // SQLite: the soft-delete flag goes out first as its own statement,
        // then the staged events (if any) through `persist_events`.
        let delete_query = crate::runtime_query::query(
            &format!(
                "UPDATE {} SET {}{}deleted = TRUE WHERE id = $1 RETURNING id",
                self.table_name,
                column_updates,
                if column_updates.is_empty() { "" } else { ", " },
            ),
            &args,
        );
        let no_snapshot = self.snapshot.then(|| quote! { , None });

        #[cfg(feature = "instrument")]
        let (instrument_attr, record_id, error_recording) = {
            let entity_name = entity.to_string();
            let repo_name = &self.repo_name_snake;
            let span_name = format!("{}.delete", repo_name);
            (
                quote! {
                    #[tracing::instrument(name = #span_name, skip_all, fields(entity = #entity_name, id = tracing::field::Empty, error = tracing::field::Empty, exception.message = tracing::field::Empty, exception.type = tracing::field::Empty))]
                },
                quote! {
                    tracing::Span::current().record("id", tracing::field::debug(&entity.id));
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
            quote! { let _ = n_events; }
        };

        let id_type = self.id;
        let events_table_name = self.events_table_name;
        // The payload purge runs *before* the events write: staged events are
        // persisted after it, so — as on Postgres — a payload staged but not
        // yet persisted at delete time survives the delete.
        let forget_payloads = if let Some(forgettable_tbl) = self.forgettable_table_name {
            let forget_query = crate::runtime_query::query(
                &format!("DELETE FROM {} WHERE entity_id = $1", forgettable_tbl),
                &[quote! { id as &#id_type }],
            );
            quote! {
                #forget_query
                .execute(op.as_executor())
                .await?;
            }
        } else {
            quote! {}
        };

        let write_error_ty = &self.write_error_ty;
        let (rejection_prelude, rejection_tail, constraint_values) = if self.can_reject {
            (
                quote! { use es_entity::errlanes::ResultExt as _; },
                quote! { .map_rejected(|r| r.with_attempted(__constraint_values))? },
                constraint_values,
            )
        } else {
            (
                quote! { use es_entity::errlanes::ResultExt as _; },
                quote! { .narrow_rejected()? },
                quote! {},
            )
        };

        let standalone = (!self.in_op_only).then(|| {
            quote! {
                pub async fn delete(
                    &self,
                    entity: #entity
                ) -> Result<(), #write_error_ty> {
                    let mut op = self.begin_op().await?;
                    let res = self.delete_in_op(&mut op, entity).await?;
                    op.commit().await?;
                    Ok(res)
                }
            }
        });

        tokens.append_all(quote! {
            #standalone

            #instrument_attr
            pub async fn delete_in_op<OP>(&self,
                op: &mut OP,
                mut entity: #entity
            ) -> Result<(), #write_error_ty>
            where
                OP: es_entity::AtomicOperation + ?Sized
            {
                let __result: Result<(), #write_error_ty> = async {
                    #rejection_prelude

                    #(#nested_deletes)*
                    #assignments
                    #constraint_values
                    #record_id

                    #forget_payloads

                    let new_events = entity.events().any_new();

                    let rows = #delete_query
                        .fetch_all(op.as_executor())
                        .await
                        .map_err(|e| Self::classify_update_write(e, format!("{} seq conflict", #table_name)))
                        #rejection_tail;

                    if new_events {
                        // No row means the UPDATE matched nothing — the
                        // entity was hard-deleted underneath us. That is a lost
                        // race, not an internal error.
                        let _ = rows
                            .first()
                            .ok_or_else(|| errlanes::Transient::new(errlanes::TransientKind::OptimisticConflict)
                                .with_context(format!("{} row vanished", #table_name))
                            )?;
                        let n_events = Self::classify_conflict(
                            self.persist_events(op, Self::extract_events(&mut entity) #no_snapshot).await,
                            #events_table_name,
                            || format!("{} seq conflict", #table_name),
                        )?;

                        #post_persist_check
                    }

                    Ok(())
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

    /// The soft-delete flag is its own SQLite statement (no data-modifying
    /// CTE); the payload purge precedes it and staged events follow through
    /// `persist_events`.
    #[test]
    fn delete_fn_soft_deletes_then_persists_events() {
        let id = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let mut columns = Columns::default();
        columns.set_id_column(&id);

        let delete_fn = DeleteFn {
            in_op_only: false,
            id: &id,
            entity: &entity,
            table_name: "entities",
            events_table_name: "entity_events",
            snapshot: false,
            columns: &columns,
            delete_option: &DeleteOption::Soft,
            nested_delete_fn_names: Vec::new(),
            post_persist_hook: false,
            can_reject: true,
            write_error_ty: quote! { es_entity::RepoWriteError<EntityConstraintViolation> },
            forgettable_table_name: Some("entities_forgettable_payloads"),
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        delete_fn.to_tokens(&mut tokens);
        let out = tokens.to_string();

        assert!(out.contains("\"UPDATE entities SET deleted = TRUE WHERE id = ?1 RETURNING id\""));
        assert!(!out.contains("WITH updated"));
        let purge_at = out
            .find("DELETE FROM entities_forgettable_payloads WHERE entity_id = ?1")
            .unwrap();
        let flag_at = out.find("UPDATE entities SET deleted").unwrap();
        let persist_at = out.find("persist_events").unwrap();
        assert!(purge_at < flag_at && flag_at < persist_at);
    }
}
