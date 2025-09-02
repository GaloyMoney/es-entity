mod input;

use convert_case::{Case, Casing};
use darling::ToTokens;
use proc_macro2::{Span, TokenStream};
use quote::{TokenStreamExt, quote};

pub use input::QueryInput;

pub fn expand(input: QueryInput) -> darling::Result<proc_macro2::TokenStream> {
    let query = EsQuery::from(input);
    Ok(quote!(#query))
}

pub struct EsQuery {
    input: QueryInput,
}

impl From<QueryInput> for EsQuery {
    fn from(input: QueryInput) -> Self {
        Self { input }
    }
}

impl ToTokens for EsQuery {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let singular = pluralizer::pluralize(
            &self
                .input
                .table_name()
                .expect("Could not identify table name"),
            1,
            false,
        );
        let entity = if let Some(entity_ty) = &self.input.entity {
            entity_ty.clone()
        } else {
            let singular_without_prefix = pluralizer::pluralize(
                &self
                    .input
                    .table_name_without_prefix()
                    .expect("Could not identify table name"),
                1,
                false,
            );
            syn::Ident::new(
                &singular_without_prefix.to_case(Case::UpperCamel),
                Span::call_site(),
            )
        };

        let entity_snake = entity.to_string().to_case(Case::Snake);
        let repo_types_mod =
            syn::Ident::new(&format!("{entity_snake}_repo_types"), Span::call_site());
        let order_by = self.input.order_by();

        let events_table = syn::Ident::new(&format!("{singular}_events"), Span::call_site());
        let args = &self.input.arg_exprs;
        let context_arg = format!("${}", args.len() + 1);

        let query = format!(
            "WITH entities AS ({}) SELECT i.id AS \"entity_id: Repo__Id\", e.sequence, e.event, CASE WHEN {} THEN e.context ELSE NULL::jsonb END as \"context: es_entity::ContextData\", e.recorded_at FROM entities i JOIN {} e ON i.id = e.id ORDER BY {} e.sequence",
            self.input.sql, context_arg, events_table, order_by
        );

        tokens.append_all(quote! {
            {
                use #repo_types_mod::*;

                es_entity::EsQuery::<Self, <Self as es_entity::EsRepo>::EsQueryFlavor, _, _>::new(
                    sqlx::query_as!(
                        Repo__DbEvent,
                        #query,
                        #(#args,)*
                        <Self as es_entity::EsRepo>::load_event_context(),
                    )
                )
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    #[test]
    fn query() {
        let input: QueryInput = parse_quote!(
            sql = "SELECT * FROM users WHERE id = $1",
            args = [id as UserId]
        );

        let query = EsQuery::from(input);
        let mut tokens = TokenStream::new();
        query.to_tokens(&mut tokens);

        let expected = quote! {
            {
                use user_repo_types::*;

                es_entity::EsQuery::<Self, <Self as es_entity::EsRepo>::EsQueryFlavor, _, _>::new(
                    sqlx::query_as!(
                        Repo__DbEvent,
                        "WITH entities AS (SELECT * FROM users WHERE id = $1) SELECT i.id AS \"entity_id: Repo__Id\", e.sequence, e.event, CASE WHEN $2 THEN e.context ELSE NULL::jsonb END as \"context: es_entity::ContextData\", e.recorded_at FROM entities i JOIN user_events e ON i.id = e.id ORDER BY i.id, e.sequence",
                        id as UserId,
                        <Self as es_entity::EsRepo>::load_event_context(),
                    )
                )
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn query_with_entity_ty() {
        let input: QueryInput = parse_quote!(
            entity = MyCustomEntity,
            sql = "SELECT * FROM my_custom_table WHERE id = $1",
            args = [id as MyCustomEntityId]
        );

        let query = EsQuery::from(input);
        let mut tokens = TokenStream::new();
        query.to_tokens(&mut tokens);

        let expected = quote! {
            {
                use my_custom_entity_repo_types::*;

                es_entity::EsQuery::<Self, <Self as es_entity::EsRepo>::EsQueryFlavor, _, _>::new(
                    sqlx::query_as!(
                        Repo__DbEvent,
                        "WITH entities AS (SELECT * FROM my_custom_table WHERE id = $1) SELECT i.id AS \"entity_id: Repo__Id\", e.sequence, e.event, CASE WHEN $2 THEN e.context ELSE NULL::jsonb END as \"context: es_entity::ContextData\", e.recorded_at FROM entities i JOIN my_custom_table_events e ON i.id = e.id ORDER BY i.id, e.sequence",
                        id as MyCustomEntityId,
                        <Self as es_entity::EsRepo>::load_event_context(),
                    )
                )
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn query_with_order() {
        let input: QueryInput = parse_quote!(
            sql = "SELECT name, id FROM entities WHERE ((name, id) > ($3, $2)) OR $2 IS NULL ORDER BY name, id LIMIT $1",
            args = [
                (first + 1) as i64,
                id as Option<MyCustomEntityId>,
                name as Option<String>
            ]
        );

        let query = EsQuery::from(input);
        let mut tokens = TokenStream::new();
        query.to_tokens(&mut tokens);

        let expected = quote! {
            {
                use entity_repo_types::*;

                es_entity::EsQuery::<Self, <Self as es_entity::EsRepo>::EsQueryFlavor, _, _>::new(
                    sqlx::query_as!(
                        Repo__DbEvent,
                        "WITH entities AS (SELECT name, id FROM entities WHERE ((name, id) > ($3, $2)) OR $2 IS NULL ORDER BY name, id LIMIT $1) SELECT i.id AS \"entity_id: Repo__Id\", e.sequence, e.event, CASE WHEN $4 THEN e.context ELSE NULL::jsonb END as \"context: es_entity::ContextData\", e.recorded_at FROM entities i JOIN entity_events e ON i.id = e.id ORDER BY i.name, i.id, i.id, e.sequence",
                        (first + 1) as i64,
                        id as Option<MyCustomEntityId>,
                        name as Option<String>,
                        <Self as es_entity::EsRepo>::load_event_context(),
                    )
                )
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
