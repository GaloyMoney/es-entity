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
        let query = format!(
            "WITH entities AS (SELECT * FROM {} WHERE ({} = ANY($1)){}) SELECT i.id AS \"entity_id: {}\", e.sequence, e.event, CASE WHEN $2 THEN e.context ELSE NULL::jsonb END as \"context: es_entity::ContextData\", e.recorded_at FROM entities i JOIN {} e ON i.id = e.id ORDER BY e.id, e.sequence",
            self.table_name,
            self.column.name(),
            not_deleted_condition,
            self.id,
            self.events_table_name,
        );

        let (impl_generics, ty_generics, where_clause) = self.generics.split_for_impl();

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
                        ).fetch_all(op.as_executor()).await?
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
            }
        });

        if self.delete_option.is_soft() {
            let column_name = self.column.name();
            let cascade_query = format!(
                "UPDATE {} SET deleted = TRUE WHERE {} = $1 AND deleted = FALSE",
                self.table_name, column_name,
            );

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
                        sqlx::query!(
                            #cascade_query,
                            parent_id as &#ty,
                        )
                        .execute(op.as_executor())
                        .await?;
                        Ok(())
                    }
                }
            });
        }
    }
}
