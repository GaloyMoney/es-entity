use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::*;

pub struct PopulateNested<'a> {
    column: &'a Column,
    ident: &'a syn::Ident,
    generics: &'a syn::Generics,
    id: &'a syn::Ident,
    table_name: &'a str,
    events_table_name: &'a str,
    repo_types_mod: syn::Ident,
    delete_option: &'a DeleteOption,
    forgettable_table_name: Option<&'a str>,
    forgettable_columns: Vec<&'a syn::Ident>,
}

impl<'a> PopulateNested<'a> {
    pub fn new(column: &'a Column, opts: &'a RepositoryOptions) -> Self {
        Self {
            column,
            ident: &opts.ident,
            generics: &opts.generics,
            id: opts.id(),
            table_name: opts.table_name(),
            events_table_name: opts.events_table_name(),
            repo_types_mod: opts.repo_types_mod(),
            delete_option: &opts.delete,
            forgettable_table_name: opts.forgettable_table_name(),
            forgettable_columns: opts.columns.forgettable_column_names(),
        }
    }
}

impl ToTokens for PopulateNested<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let ty = self.column.ty();
        let ident = self.ident;
        let repo_types_mod = &self.repo_types_mod;
        let accessor = self.column.parent_accessor();

        let not_deleted_condition = self.delete_option.not_deleted_condition();
        let (payload_column, forgettable_join) =
            if let Some(forgettable_tbl) = self.forgettable_table_name {
                (
                    "p.payload as \"forgettable_payload?\"".to_string(),
                    format!(
                        " LEFT JOIN {} p ON e.id = p.entity_id AND e.sequence = p.sequence",
                        forgettable_tbl
                    ),
                )
            } else {
                (
                    "NULL::jsonb as \"forgettable_payload?\"".to_string(),
                    String::new(),
                )
            };

        let query = format!(
            "WITH entities AS (SELECT * FROM {} WHERE ({} = ANY($1)){}) SELECT i.id AS \"entity_id: {}\", e.sequence, e.event, CASE WHEN $2 THEN e.context ELSE NULL::jsonb END as \"context: es_entity::ContextData\", e.recorded_at, {} FROM entities i JOIN {} e ON i.id = e.id{} ORDER BY e.id, e.sequence",
            self.table_name,
            self.column.name(),
            not_deleted_condition,
            self.id,
            payload_column,
            self.events_table_name,
            forgettable_join,
        );

        let (impl_generics, ty_generics, where_clause) = self.generics.split_for_impl();

        let include_deleted_override = if self.delete_option.is_soft() {
            let include_deleted_query = format!(
                "WITH entities AS (SELECT * FROM {} WHERE ({} = ANY($1))) SELECT i.id AS \"entity_id: {}\", e.sequence, e.event, CASE WHEN $2 THEN e.context ELSE NULL::jsonb END as \"context: es_entity::ContextData\", e.recorded_at, {} FROM entities i JOIN {} e ON i.id = e.id{} ORDER BY e.id, e.sequence",
                self.table_name,
                self.column.name(),
                self.id,
                payload_column,
                self.events_table_name,
                forgettable_join,
            );
            quote! {
                async fn populate_in_op_include_deleted<OP, P, __EsErr>(
                    op: &mut OP,
                    mut lookup: std::collections::HashMap<#ty, &mut P>,
                ) -> Result<(), __EsErr>
                where
                    OP: es_entity::AtomicOperation,
                    P: Parent<<Self as EsRepo>::Entity>,
                    __EsErr: From<sqlx::Error> + From<es_entity::EntityHydrationError> + Send,
                {
                    let parent_ids: Vec<_> = lookup.keys().collect();
                    let rows = {
                        sqlx::query_as!(
                            #repo_types_mod::Repo__DbEvent,
                            #include_deleted_query,
                            parent_ids.as_slice() as &[&#ty],
                            <#repo_types_mod::Repo__Event as EsEvent>::event_context(),
                        ).fetch_all(es_entity::annotate_executor(op.as_executor())).await?
                    };
                    let n = rows.len();
                    let (mut res, _) = es_entity::EntityEvents::load_n::<<Self as EsRepo>::Entity>(rows.into_iter(), n)?;
                    Self::load_all_nested_in_op_include_deleted::<_, __EsErr>(op, &mut res).await?;
                    for entity in res.into_iter() {
                        let parent = lookup.get_mut(&entity.#accessor).expect("parent not present");
                        parent.inject_children(std::iter::once(entity));
                    }
                    Ok(())
                }
            }
        } else {
            quote! {}
        };

        tokens.append_all(quote! {
            impl #impl_generics es_entity::PopulateNested<#ty> for #ident #ty_generics #where_clause {
                async fn populate_in_op<OP, P, __EsErr>(
                    op: &mut OP,
                    mut lookup: std::collections::HashMap<#ty, &mut P>,
                ) -> Result<(), __EsErr>
                where
                    OP: es_entity::AtomicOperation,
                    P: Parent<<Self as EsRepo>::Entity>,
                    __EsErr: From<sqlx::Error> + From<es_entity::EntityHydrationError> + Send,
                {
                    let parent_ids: Vec<_> = lookup.keys().collect();
                    let rows = {
                        sqlx::query_as!(
                            #repo_types_mod::Repo__DbEvent,
                            #query,
                            parent_ids.as_slice() as &[&#ty],
                            <#repo_types_mod::Repo__Event as EsEvent>::event_context(),
                        ).fetch_all(es_entity::annotate_executor(op.as_executor())).await?
                    };
                    let n = rows.len();
                    let (mut res, _) = es_entity::EntityEvents::load_n::<<Self as EsRepo>::Entity>(rows.into_iter(), n)?;
                    Self::load_all_nested_in_op::<_, __EsErr>(op, &mut res).await?;
                    for entity in res.into_iter() {
                        let parent = lookup.get_mut(&entity.#accessor).expect("parent not present");
                        parent.inject_children(std::iter::once(entity));
                    }
                    Ok(())
                }

                #include_deleted_override
            }
        });

        if self.delete_option.is_soft() {
            let column_name = self.column.name();

            let cascade = if let Some(forgettable_tbl) = self.forgettable_table_name {
                // Scrub the direct nested children's forgettable data before the
                // soft-delete flips them, mirroring the parent's own delete
                // scrub (scoped to direct children by the parent FK): delete the
                // child payload rows first, then NULL any child forgettable index
                // columns in the same UPDATE that sets `deleted = TRUE`.
                let payload_delete_query = format!(
                    "DELETE FROM {} WHERE entity_id IN (SELECT id FROM {} WHERE {} = $1)",
                    forgettable_tbl, self.table_name, column_name,
                );
                let null_cols = self
                    .forgettable_columns
                    .iter()
                    .map(|c| format!(", {} = NULL", c))
                    .collect::<String>();
                let cascade_query = format!(
                    "UPDATE {} SET deleted = TRUE{} WHERE {} = $1 AND deleted = FALSE",
                    self.table_name, null_cols, column_name,
                );
                quote! {
                    sqlx::query!(
                        #payload_delete_query,
                        parent_id as &#ty,
                    )
                    .execute(es_entity::annotate_executor(op.as_executor()))
                    .await?;
                    sqlx::query!(
                        #cascade_query,
                        parent_id as &#ty,
                    )
                    .execute(es_entity::annotate_executor(op.as_executor()))
                    .await?;
                }
            } else {
                let cascade_query = format!(
                    "UPDATE {} SET deleted = TRUE WHERE {} = $1 AND deleted = FALSE",
                    self.table_name, column_name,
                );
                quote! {
                    sqlx::query!(
                        #cascade_query,
                        parent_id as &#ty,
                    )
                    .execute(es_entity::annotate_executor(op.as_executor()))
                    .await?;
                }
            };

            tokens.append_all(quote! {
                impl #impl_generics es_entity::CascadeDeleteNested<#ty> for #ident #ty_generics #where_clause {
                    async fn cascade_delete_in_op<OP, __EsErr>(
                        op: &mut OP,
                        parent_id: &#ty,
                    ) -> Result<(), __EsErr>
                    where
                        OP: es_entity::AtomicOperation,
                        __EsErr: From<sqlx::Error> + Send,
                    {
                        #cascade
                        Ok(())
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use darling::FromDeriveInput;
    use syn::parse_quote;

    use super::*;
    use crate::repo::options::RepositoryOptions;

    fn cascade_output(input: syn::DeriveInput) -> String {
        let opts = RepositoryOptions::from_derive_input(&input).unwrap();
        let parent = opts
            .columns
            .parent()
            .expect("repo must declare a parent column");
        let mut tokens = TokenStream::new();
        PopulateNested::new(parent, &opts).to_tokens(&mut tokens);
        tokens.to_string()
    }

    #[test]
    fn cascade_scrubs_forgettable_nested_children() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(
                entity = "AccountHolder",
                forgettable,
                delete = "soft",
                columns(
                    account_id(ty = "AccountId", update(persist = false), parent),
                    email(ty = "Forgettable<String>")
                )
            )]
            struct AccountHolders {
                pool: sqlx::PgPool,
            }
        };
        let output = cascade_output(input);
        // Child payload rows are deleted first, scoped to direct children by
        // the parent FK.
        assert!(output.contains(
            "DELETE FROM account_holders_forgettable_payloads WHERE entity_id IN (SELECT id FROM account_holders WHERE account_id = $1)"
        ));
        // The soft-delete UPDATE also NULLs the child forgettable index column.
        assert!(output.contains(
            "UPDATE account_holders SET deleted = TRUE, email = NULL WHERE account_id = $1 AND deleted = FALSE"
        ));
    }

    #[test]
    fn cascade_leaves_non_forgettable_children_unchanged() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(
                entity = "AccountHolder",
                delete = "soft",
                columns(
                    account_id(ty = "AccountId", update(persist = false), parent),
                    label(ty = "String")
                )
            )]
            struct AccountHolders {
                pool: sqlx::PgPool,
            }
        };
        let output = cascade_output(input);
        assert!(output.contains(
            "UPDATE account_holders SET deleted = TRUE WHERE account_id = $1 AND deleted = FALSE"
        ));
        assert!(!output.contains("DELETE FROM"));
        assert!(!output.contains("= NULL"));
    }
}
