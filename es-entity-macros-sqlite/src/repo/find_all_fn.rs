use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::*;

pub struct FindAllFn<'a> {
    id: &'a syn::Ident,
    entity: &'a syn::Ident,
    table_name: &'a str,
    events_table_name: &'a str,
    query_error: syn::Ident,
    any_nested: bool,
    post_hydrate_error: Option<&'a syn::Type>,
    repo_types_mod: syn::Ident,
    #[cfg(feature = "instrument")]
    repo_name_snake: String,
}

impl<'a> From<&'a RepositoryOptions> for FindAllFn<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            id: opts.id(),
            entity: opts.entity(),
            table_name: opts.table_name(),
            events_table_name: opts.events_table_name(),
            query_error: opts.query_error(),
            any_nested: opts.any_nested(),
            post_hydrate_error: opts.post_hydrate_hook.as_ref().map(|h| &h.error),
            repo_types_mod: opts.repo_types_mod(),
            #[cfg(feature = "instrument")]
            repo_name_snake: opts.repo_name_snake_case(),
        }
    }
}

impl ToTokens for FindAllFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let id = self.id;
        let entity = self.entity;
        let query_error = &self.query_error;
        let query_fn_op_traits = RepositoryOptions::query_fn_op_traits(self.any_nested);
        let query_fn_get_op = RepositoryOptions::query_fn_get_op(self.any_nested);
        let repo_types_mod = &self.repo_types_mod;
        let table_name = self.table_name;
        let events_table_name = self.events_table_name;

        let generics = if self.any_nested {
            quote! { <Out: From<#entity>> }
        } else {
            quote! { <'a, Out: From<#entity>> }
        };

        let op_param = if self.any_nested {
            quote! { op: &mut impl #query_fn_op_traits }
        } else {
            quote! { op: impl #query_fn_op_traits }
        };

        #[cfg(feature = "instrument")]
        let instrument_attr = {
            let entity_name = entity.to_string();
            let repo_name = &self.repo_name_snake;
            let span_name = format!("{}.find_all", repo_name);
            quote! {
                #[tracing::instrument(name = #span_name, skip_all, fields(entity = #entity_name, count = ids.len(), ids = tracing::field::debug(ids)), err)]
            }
        };
        #[cfg(not(feature = "instrument"))]
        let instrument_attr = quote! {};

        let post_hydrate_check = if self.post_hydrate_error.is_some() {
            quote! {
                for __entity in &entities {
                    self.execute_post_hydrate_hook(__entity).map_err(#query_error::PostHydrateError)?;
                }
            }
        } else {
            quote! {}
        };

        let fetch_and_load = if self.any_nested {
            quote! {
                let db_events = (&mut *op).into_executor().fetch_all(query).await?;
                let n = db_events.len();
                let (mut entities, _) = es_entity::EntityEvents::load_n::<#entity>(db_events.into_iter(), n)?;
                Self::load_all_nested_in_op::<_, #query_error>(op, &mut entities).await?;
            }
        } else {
            quote! {
                let db_events = op.into_executor().fetch_all(query).await?;
                let n = db_events.len();
                let (mut entities, _) = es_entity::EntityEvents::load_n::<#entity>(db_events.into_iter(), n)?;
            }
        };

        tokens.append_all(quote! {
            pub async fn find_all<Out: From<#entity>>(
                &self,
                ids: &[#id]
            ) -> Result<std::collections::HashMap<#id, Out>, #query_error> {
                self.find_all_in_op(#query_fn_get_op, ids).await
            }

            #instrument_attr
            pub async fn find_all_in_op #generics(
                &self,
                #op_param,
                ids: &[#id]
            ) -> Result<std::collections::HashMap<#id, Out>, #query_error> {
                if ids.is_empty() {
                    return Ok(std::collections::HashMap::new());
                }
                let placeholders: String = (1..=ids.len())
                    .map(|i| format!("?{i}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let ctx_param = ids.len() + 1;
                let query_str = format!(
                    "WITH entities AS (SELECT * FROM {} WHERE id IN ({})) \
                     SELECT i.id AS entity_id, e.sequence, e.event, \
                     CASE WHEN ?{} THEN e.context ELSE NULL END AS context, \
                     e.recorded_at \
                     FROM entities i JOIN {} e ON i.id = e.id ORDER BY e.id, e.sequence",
                    #table_name,
                    placeholders,
                    ctx_param,
                    #events_table_name,
                );
                let mut query = es_entity::prelude::sqlx::query(&query_str);
                for id in ids {
                    query = query.bind(id);
                }
                query = query.bind(<#repo_types_mod::Repo__Event as EsEvent>::event_context());
                let query = query.try_map(|row: es_entity::db::Row| -> Result<#repo_types_mod::Repo__DbEvent, sqlx::Error> {
                    use es_entity::prelude::sqlx::Row as _;
                    Ok(#repo_types_mod::Repo__DbEvent {
                        entity_id: row.try_get("entity_id")?,
                        sequence: row.try_get("sequence")?,
                        event: row.try_get("event")?,
                        context: row.try_get("context")?,
                        recorded_at: row.try_get("recorded_at")?,
                    })
                });
                #fetch_and_load
                #post_hydrate_check
                Ok(entities.into_iter().map(|u| (u.id.clone(), Out::from(u))).collect())
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
    fn find_all_fn() {
        let id_type = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let query_error = syn::Ident::new("EntityQueryError", Span::call_site());

        let persist_fn = FindAllFn {
            id: &id_type,
            entity: &entity,
            table_name: "entities",
            events_table_name: "entity_events",
            query_error,
            any_nested: false,
            post_hydrate_error: None,
            repo_types_mod: syn::Ident::new("entity_repo_types", Span::call_site()),
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn find_all<Out: From<Entity>>(
                &self,
                ids: &[EntityId]
            ) -> Result<std::collections::HashMap<EntityId, Out>, EntityQueryError> {
                self.find_all_in_op(self.pool(), ids).await
            }

            pub async fn find_all_in_op<'a, Out: From<Entity>>(
                &self,
                op: impl es_entity::IntoOneTimeExecutor<'a>,
                ids: &[EntityId]
            ) -> Result<std::collections::HashMap<EntityId, Out>, EntityQueryError> {
                if ids.is_empty() {
                    return Ok(std::collections::HashMap::new());
                }
                let placeholders: String = (1..=ids.len())
                    .map(|i| format!("?{i}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let ctx_param = ids.len() + 1;
                let query_str = format!(
                    "WITH entities AS (SELECT * FROM {} WHERE id IN ({})) \
                     SELECT i.id AS entity_id, e.sequence, e.event, \
                     CASE WHEN ?{} THEN e.context ELSE NULL END AS context, \
                     e.recorded_at \
                     FROM entities i JOIN {} e ON i.id = e.id ORDER BY e.id, e.sequence",
                    "entities",
                    placeholders,
                    ctx_param,
                    "entity_events",
                );
                let mut query = es_entity::prelude::sqlx::query(&query_str);
                for id in ids {
                    query = query.bind(id);
                }
                query = query.bind(<entity_repo_types::Repo__Event as EsEvent>::event_context());
                let query = query.try_map(|row: es_entity::db::Row| -> Result<entity_repo_types::Repo__DbEvent, sqlx::Error> {
                    use es_entity::prelude::sqlx::Row as _;
                    Ok(entity_repo_types::Repo__DbEvent {
                        entity_id: row.try_get("entity_id")?,
                        sequence: row.try_get("sequence")?,
                        event: row.try_get("event")?,
                        context: row.try_get("context")?,
                        recorded_at: row.try_get("recorded_at")?,
                    })
                });
                let db_events = op.into_executor().fetch_all(query).await?;
                let n = db_events.len();
                let (mut entities, _) = es_entity::EntityEvents::load_n::<Entity>(db_events.into_iter(), n)?;
                Ok(entities.into_iter().map(|u| (u.id.clone(), Out::from(u))).collect())
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
