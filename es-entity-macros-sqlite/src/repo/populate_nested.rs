use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::*;

pub struct PopulateNested<'a> {
    column: &'a Column,
    ident: &'a syn::Ident,
    generics: &'a syn::Generics,
    table_name: &'a str,
    events_table_name: &'a str,
    repo_types_mod: syn::Ident,
}

impl<'a> PopulateNested<'a> {
    pub fn new(column: &'a Column, opts: &'a RepositoryOptions) -> Self {
        Self {
            column,
            ident: &opts.ident,
            generics: &opts.generics,
            table_name: opts.table_name(),
            events_table_name: opts.events_table_name(),
            repo_types_mod: opts.repo_types_mod(),
        }
    }
}

impl ToTokens for PopulateNested<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let ty = self.column.ty();
        let ident = self.ident;
        let repo_types_mod = &self.repo_types_mod;
        let accessor = self.column.parent_accessor();
        let table_name = self.table_name;
        let column_name = self.column.name().to_string();
        let events_table_name = self.events_table_name;

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
                    if parent_ids.is_empty() {
                        return Ok(());
                    }
                    let placeholders: String = (1..=parent_ids.len())
                        .map(|i| format!("?{i}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let ctx_param = parent_ids.len() + 1;
                    let query_str = format!(
                        "WITH entities AS (SELECT * FROM {} WHERE ({} IN ({}))) \
                         SELECT i.id AS entity_id, e.sequence, e.event, \
                         CASE WHEN ?{} THEN e.context ELSE NULL END AS context, \
                         e.recorded_at \
                         FROM entities i JOIN {} e ON i.id = e.id ORDER BY e.id, e.sequence",
                        #table_name,
                        #column_name,
                        placeholders,
                        ctx_param,
                        #events_table_name,
                    );
                    let mut query = es_entity::prelude::sqlx::query(&query_str);
                    for id in &parent_ids {
                        query = query.bind(id);
                    }
                    query = query.bind(<#repo_types_mod::Repo__Event as EsEvent>::event_context());
                    let rows = query.fetch_all(op.as_executor()).await?;
                    use es_entity::prelude::sqlx::Row as _;
                    let db_events: Vec<#repo_types_mod::Repo__DbEvent> = rows.iter().map(|row| {
                        #repo_types_mod::Repo__DbEvent {
                            entity_id: row.try_get("entity_id").expect("entity_id"),
                            sequence: row.try_get("sequence").expect("sequence"),
                            event: row.try_get("event").expect("event"),
                            context: row.try_get("context").expect("context"),
                            recorded_at: row.try_get("recorded_at").expect("recorded_at"),
                        }
                    }).collect();
                    let n = db_events.len();
                    let (mut res, _) = es_entity::EntityEvents::load_n::<<Self as EsRepo>::Entity>(db_events.into_iter(), n)?;
                    Self::load_all_nested_in_op::<_, __EsErr>(op, &mut res).await?;
                    for entity in res.into_iter() {
                        let parent = lookup.get_mut(&entity.#accessor).expect("parent not present");
                        parent.inject_children(std::iter::once(entity));
                    }
                    Ok(())
                }
            }
        });
    }
}
