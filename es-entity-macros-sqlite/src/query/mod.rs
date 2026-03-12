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

/// Convert `$N` bind parameters in SQL to SQLite-style `?N`.
fn pg_to_sqlite_params(sql: &str) -> String {
    use regex::Regex;
    let re = Regex::new(r"\$(\d+)").unwrap();
    re.replace_all(sql, "?$1").to_string()
}

/// Strip sqlx-style type annotations (`as CustomType`) from a cast
/// expression while preserving actual Rust primitive casts (`as i64`).
///
/// In `query_as!`, `id as UserId` is a type annotation for sqlx, not a
/// Rust cast.  We strip those.  But `(first + 1) as i64` is a genuine
/// type conversion we must keep.
///
/// Heuristic: if the target type is a single-segment path whose ident
/// starts with a lowercase letter, it's a primitive cast – keep it.
/// Otherwise strip the cast.
fn strip_cast(expr: &syn::Expr) -> &syn::Expr {
    match expr {
        syn::Expr::Cast(cast) => {
            if is_primitive_cast(&cast.ty) {
                expr // keep `(first + 1) as i64`
            } else {
                &cast.expr // strip `id as UserId`
            }
        }
        other => other,
    }
}

fn is_primitive_cast(ty: &syn::Type) -> bool {
    if let syn::Type::Path(path) = ty {
        if let Some(ident) = path.path.get_ident() {
            let s = ident.to_string();
            s.starts_with(|c: char| c.is_ascii_lowercase())
        } else {
            false
        }
    } else {
        false
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

        let events_table = format!("{singular}_events");
        let args = &self.input.arg_exprs;
        let context_arg_num = args.len() + 1;

        // Convert $N to ?N in the user-provided SQL
        let user_sql = pg_to_sqlite_params(&self.input.sql);

        let query = format!(
            "WITH entities AS ({}) SELECT i.id AS entity_id, e.sequence, e.event, CASE WHEN ?{} THEN e.context ELSE NULL END AS context, e.recorded_at FROM {} e JOIN entities i ON i.id = e.id ORDER BY {} e.sequence",
            user_sql, context_arg_num, events_table, order_by
        );

        // Generate .bind() calls for each arg, stripping `as Type` casts
        let bind_exprs: Vec<&syn::Expr> = args.iter().map(strip_cast).collect();

        tokens.append_all(quote! {
            {
                use #repo_types_mod::*;
                use es_entity::prelude::sqlx::Row as _;

                es_entity::EsQuery::<Self, <Self as es_entity::EsRepo>::EsQueryFlavor, _, _>::new(
                    sqlx::query(#query)
                        #(.bind(#bind_exprs))*
                        .bind(<<<Self as es_entity::EsRepo>::Entity as EsEntity>::Event>::event_context())
                        .try_map(|row: es_entity::db::Row| -> Result<Repo__DbEvent, sqlx::Error> {
                            Ok(Repo__DbEvent {
                                entity_id: row.try_get("entity_id")?,
                                sequence: row.try_get("sequence")?,
                                event: row.try_get("event")?,
                                context: row.try_get("context")?,
                                recorded_at: row.try_get("recorded_at")?,
                            })
                        })
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
                use es_entity::prelude::sqlx::Row as _;

                es_entity::EsQuery::<Self, <Self as es_entity::EsRepo>::EsQueryFlavor, _, _>::new(
                    sqlx::query("WITH entities AS (SELECT * FROM users WHERE id = ?1) SELECT i.id AS entity_id, e.sequence, e.event, CASE WHEN ?2 THEN e.context ELSE NULL END AS context, e.recorded_at FROM user_events e JOIN entities i ON i.id = e.id ORDER BY i.id, e.sequence")
                        .bind(id)
                        .bind(<<<Self as es_entity::EsRepo>::Entity as EsEntity>::Event>::event_context())
                        .try_map(|row: es_entity::db::Row| -> Result<Repo__DbEvent, sqlx::Error> {
                            Ok(Repo__DbEvent {
                                entity_id: row.try_get("entity_id")?,
                                sequence: row.try_get("sequence")?,
                                event: row.try_get("event")?,
                                context: row.try_get("context")?,
                                recorded_at: row.try_get("recorded_at")?,
                            })
                        })
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
                use es_entity::prelude::sqlx::Row as _;

                es_entity::EsQuery::<Self, <Self as es_entity::EsRepo>::EsQueryFlavor, _, _>::new(
                    sqlx::query("WITH entities AS (SELECT * FROM my_custom_table WHERE id = ?1) SELECT i.id AS entity_id, e.sequence, e.event, CASE WHEN ?2 THEN e.context ELSE NULL END AS context, e.recorded_at FROM my_custom_table_events e JOIN entities i ON i.id = e.id ORDER BY i.id, e.sequence")
                        .bind(id)
                        .bind(<<<Self as es_entity::EsRepo>::Entity as EsEntity>::Event>::event_context())
                        .try_map(|row: es_entity::db::Row| -> Result<Repo__DbEvent, sqlx::Error> {
                            Ok(Repo__DbEvent {
                                entity_id: row.try_get("entity_id")?,
                                sequence: row.try_get("sequence")?,
                                event: row.try_get("event")?,
                                context: row.try_get("context")?,
                                recorded_at: row.try_get("recorded_at")?,
                            })
                        })
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
                use es_entity::prelude::sqlx::Row as _;

                es_entity::EsQuery::<Self, <Self as es_entity::EsRepo>::EsQueryFlavor, _, _>::new(
                    sqlx::query("WITH entities AS (SELECT name, id FROM entities WHERE ((name, id) > (?3, ?2)) OR ?2 IS NULL ORDER BY name, id LIMIT ?1) SELECT i.id AS entity_id, e.sequence, e.event, CASE WHEN ?4 THEN e.context ELSE NULL END AS context, e.recorded_at FROM entity_events e JOIN entities i ON i.id = e.id ORDER BY i.name, i.id, i.id, e.sequence")
                        .bind((first + 1) as i64)
                        .bind(id)
                        .bind(name)
                        .bind(<<<Self as es_entity::EsRepo>::Entity as EsEntity>::Event>::event_context())
                        .try_map(|row: es_entity::db::Row| -> Result<Repo__DbEvent, sqlx::Error> {
                            Ok(Repo__DbEvent {
                                entity_id: row.try_get("entity_id")?,
                                sequence: row.try_get("sequence")?,
                                event: row.try_get("event")?,
                                context: row.try_get("context")?,
                                recorded_at: row.try_get("recorded_at")?,
                            })
                        })
                )
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
