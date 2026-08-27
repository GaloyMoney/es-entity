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

        let (payload_column, forgettable_join) =
            if let Some(ref forgettable_tbl) = self.input.forgettable_tbl {
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

        // Aggregate roots additionally select the index row's `version`, in the
        // same statement as the events so both are read under one snapshot —
        // that single-statement capture is what makes the read-side bracket
        // sound.
        //
        // The version comes from a join back to the root table rather than from
        // the `entities` CTE: generated queries project `SELECT id FROM <tbl>`,
        // so `version` is not in the CTE's column list. Joining on the primary
        // key of a table the statement is already reading is cheap, and it
        // keeps every call site working regardless of what its SQL projects.
        //
        // `query_as!` binds columns to fields positionally, so this trails the
        // forgettable payload to match `RootGenericEvent`'s field order. The
        // `version!` non-null override is needed for the same reason as
        // `entity_id!` below.
        let (version_column, version_join) = if self.input.root {
            let root_table = self
                .input
                .table_name()
                .expect("Could not identify table name");
            (
                ", r.version AS \"version!: i32\"".to_string(),
                format!(" JOIN {} r ON r.id = i.id", root_table),
            )
        } else {
            (String::new(), String::new())
        };

        // `entity_id!` forces the non-null assertion: `i.id` is a primary-key
        // join key so it is always non-null, but sqlx cannot infer that through
        // a `UNION ALL` CTE (the unified cursor queries) and would otherwise
        // decode it as `Option<Repo__Id>`, breaking `Repo__DbEvent`. The
        // override is a safe no-op for the non-union queries.
        let query = format!(
            "WITH entities AS ({}) SELECT i.id AS \"entity_id!: Repo__Id\", e.sequence, e.event, CASE WHEN {} THEN e.context ELSE NULL::jsonb END as \"context: es_entity::ContextData\", e.recorded_at, {}{} FROM entities i JOIN {} e ON i.id = e.id{}{} ORDER BY {} e.sequence",
            self.input.sql,
            context_arg,
            payload_column,
            version_column,
            events_table,
            forgettable_join,
            version_join,
            order_by
        );

        let forgettable_check = if self.input.forgettable_tbl.is_none() {
            quote! {
                const _: () = assert!(
                    !Repo__Event::HAS_FORGETTABLE_FIELDS,
                    "es_query! requires `forgettable_tbl` parameter when the event type has Forgettable<T> fields"
                );
            }
        } else {
            quote! {}
        };

        let tbl_prefix_check = if self.input.tbl_prefix.is_none() && self.input.entity.is_none() {
            quote! {
                const _: () = assert!(
                    !REPO__HAS_TBL_PREFIX,
                    "es_query! requires `tbl_prefix` parameter when the repo uses tbl_prefix"
                );
            }
        } else {
            quote! {}
        };

        // Root-ness and the `root` marker must agree in both directions: a root
        // repo whose query omits `version` would hydrate through
        // `RootGenericEvent` without a version to put in it, and a non-root repo
        // passing `root` would select a column its table does not have.
        let root_check = if self.input.entity.is_some() {
            // `entity = ...` queries target another repo's types mod, where
            // `REPO__IS_ROOT` describes that repo rather than this call site.
            quote! {}
        } else if self.input.root {
            quote! {
                const _: () = assert!(
                    REPO__IS_ROOT,
                    "es_query! `root` parameter is only valid on an aggregate-root repo (one with #[es_repo(nested)] fields and no `parent` column)"
                );
            }
        } else {
            quote! {
                const _: () = assert!(
                    !REPO__IS_ROOT,
                    "es_query! on an aggregate-root repo requires the `root` parameter (the query must select i.version)"
                );
            }
        };

        tokens.append_all(quote! {
            {
                use #repo_types_mod::*;

                #forgettable_check
                #tbl_prefix_check
                #root_check

                es_entity::EsQuery::<Self, <Self as es_entity::EsRepo>::EsQueryFlavor, _, _>::new(
                    sqlx::query_as!(
                        Repo__DbEvent,
                        #query,
                        #(#args,)*
                        <<<Self as es_entity::EsRepo>::Entity as EsEntity>::Event>::event_context(),
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

                const _: () = assert!(
                    !Repo__Event::HAS_FORGETTABLE_FIELDS,
                    "es_query! requires `forgettable_tbl` parameter when the event type has Forgettable<T> fields"
                );
                const _: () = assert!(
                    !REPO__HAS_TBL_PREFIX,
                    "es_query! requires `tbl_prefix` parameter when the repo uses tbl_prefix"
                );
                const _: () = assert!(
                    !REPO__IS_ROOT,
                    "es_query! on an aggregate-root repo requires the `root` parameter (the query must select i.version)"
                );

                es_entity::EsQuery::<Self, <Self as es_entity::EsRepo>::EsQueryFlavor, _, _>::new(
                    sqlx::query_as!(
                        Repo__DbEvent,
                        "WITH entities AS (SELECT * FROM users WHERE id = $1) SELECT i.id AS \"entity_id!: Repo__Id\", e.sequence, e.event, CASE WHEN $2 THEN e.context ELSE NULL::jsonb END as \"context: es_entity::ContextData\", e.recorded_at, NULL::jsonb as \"forgettable_payload?\" FROM entities i JOIN user_events e ON i.id = e.id ORDER BY i.id, e.sequence",
                        id as UserId,
                        <<<Self as es_entity::EsRepo>::Entity as EsEntity>::Event>::event_context(),
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

                const _: () = assert!(
                    !Repo__Event::HAS_FORGETTABLE_FIELDS,
                    "es_query! requires `forgettable_tbl` parameter when the event type has Forgettable<T> fields"
                );

                es_entity::EsQuery::<Self, <Self as es_entity::EsRepo>::EsQueryFlavor, _, _>::new(
                    sqlx::query_as!(
                        Repo__DbEvent,
                        "WITH entities AS (SELECT * FROM my_custom_table WHERE id = $1) SELECT i.id AS \"entity_id!: Repo__Id\", e.sequence, e.event, CASE WHEN $2 THEN e.context ELSE NULL::jsonb END as \"context: es_entity::ContextData\", e.recorded_at, NULL::jsonb as \"forgettable_payload?\" FROM entities i JOIN my_custom_table_events e ON i.id = e.id ORDER BY i.id, e.sequence",
                        id as MyCustomEntityId,
                        <<<Self as es_entity::EsRepo>::Entity as EsEntity>::Event>::event_context(),
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

                const _: () = assert!(
                    !Repo__Event::HAS_FORGETTABLE_FIELDS,
                    "es_query! requires `forgettable_tbl` parameter when the event type has Forgettable<T> fields"
                );
                const _: () = assert!(
                    !REPO__HAS_TBL_PREFIX,
                    "es_query! requires `tbl_prefix` parameter when the repo uses tbl_prefix"
                );
                const _: () = assert!(
                    !REPO__IS_ROOT,
                    "es_query! on an aggregate-root repo requires the `root` parameter (the query must select i.version)"
                );

                es_entity::EsQuery::<Self, <Self as es_entity::EsRepo>::EsQueryFlavor, _, _>::new(
                    sqlx::query_as!(
                        Repo__DbEvent,
                        "WITH entities AS (SELECT name, id FROM entities WHERE ((name, id) > ($3, $2)) OR $2 IS NULL ORDER BY name, id LIMIT $1) SELECT i.id AS \"entity_id!: Repo__Id\", e.sequence, e.event, CASE WHEN $4 THEN e.context ELSE NULL::jsonb END as \"context: es_entity::ContextData\", e.recorded_at, NULL::jsonb as \"forgettable_payload?\" FROM entities i JOIN entity_events e ON i.id = e.id ORDER BY i.name, i.id, i.id, e.sequence",
                        (first + 1) as i64,
                        id as Option<MyCustomEntityId>,
                        name as Option<String>,
                        <<<Self as es_entity::EsRepo>::Entity as EsEntity>::Event>::event_context(),
                    )
                )
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
