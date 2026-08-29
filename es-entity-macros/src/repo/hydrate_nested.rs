use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::*;

pub struct HydrateNested<'a> {
    column: &'a Column,
    ident: &'a syn::Ident,
    generics: &'a syn::Generics,
    id: &'a syn::Ident,
    table_name: &'a str,
    delete_option: &'a DeleteOption,
    forgettable_table_name: Option<&'a str>,
    forgettable_columns: Vec<&'a syn::Ident>,
}

impl<'a> HydrateNested<'a> {
    pub fn new(column: &'a Column, opts: &'a RepositoryOptions) -> Self {
        Self {
            column,
            ident: &opts.ident,
            generics: &opts.generics,
            id: opts.id(),
            table_name: opts.table_name(),
            delete_option: &opts.delete,
            forgettable_table_name: opts.forgettable_table_name(),
            forgettable_columns: opts.columns.forgettable_column_names(),
        }
    }
}

impl ToTokens for HydrateNested<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let ty = self.column.ty();
        let ident = self.ident;
        let id = self.id;
        let accessor = self.column.parent_accessor();

        let (impl_generics, ty_generics, where_clause) = self.generics.split_for_impl();

        tokens.append_all(quote! {
            impl #impl_generics es_entity::HydrateNested<#ty> for #ident #ty_generics #where_clause {
                fn hydrate_in_op<P, __EsErr>(
                    rows_by_tag: &mut std::collections::HashMap<i32, Vec<es_entity::db::Row>>,
                    tag_cursor: &mut i32,
                    mut lookup: std::collections::HashMap<#ty, &mut P>,
                ) -> Result<(), __EsErr>
                where
                    P: Parent<<Self as EsRepo>::Entity>,
                    __EsErr: From<sqlx::Error> + From<es_entity::EntityHydrationError>,
                {
                    let my_tag = *tag_cursor;
                    *tag_cursor += 1;
                    let rows = rows_by_tag.remove(&my_tag).unwrap_or_default();
                    let n = rows.len();
                    let generic = rows
                        .iter()
                        .map(es_entity::decode_tagged_row::<#id>)
                        .collect::<Result<Vec<_>, _>>()?;
                    let (mut res, _) = es_entity::EntityEvents::load_n::<<Self as EsRepo>::Entity>(generic.into_iter(), n)?;
                    <Self as es_entity::EsRepo>::hydrate_nested_from_rows::<__EsErr>(rows_by_tag, tag_cursor, &mut res)?;
                    for entity in res.into_iter() {
                        if let Some(parent) = lookup.get_mut(&entity.#accessor) {
                            parent.inject_children(std::iter::once(entity));
                        }
                    }
                    Ok(())
                }
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
                    .execute(op.as_executor())
                    .await?;
                    sqlx::query!(
                        #cascade_query,
                        parent_id as &#ty,
                    )
                    .execute(op.as_executor())
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
                    .execute(op.as_executor())
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

    fn output(input: syn::DeriveInput) -> String {
        let opts = RepositoryOptions::from_derive_input(&input).unwrap();
        let parent = opts
            .columns
            .parent()
            .expect("repo must declare a parent column");
        let mut tokens = TokenStream::new();
        HydrateNested::new(parent, &opts).to_tokens(&mut tokens);
        tokens.to_string()
    }

    #[test]
    fn hydrate_in_op_demuxes_by_tag_and_recurses_before_injecting() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(
                entity = "OrderItem",
                columns(order_id(ty = "OrderId", update(persist = false), parent))
            )]
            struct OrderItems {
                pool: sqlx::PgPool,
            }
        };
        let out = output(input);
        assert!(out.contains("impl es_entity :: HydrateNested < OrderId > for OrderItems"));
        assert!(out.contains("fn hydrate_in_op"));
        assert!(!out.contains("sqlx :: query"));
        assert!(out.contains("es_entity :: decode_tagged_row :: < OrderItemId >"));
        assert!(out.contains("hydrate_nested_from_rows"));
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
        let out = output(input);
        // Child payload rows are deleted first, scoped to direct children by
        // the parent FK.
        assert!(out.contains(
            "DELETE FROM account_holders_forgettable_payloads WHERE entity_id IN (SELECT id FROM account_holders WHERE account_id = $1)"
        ));
        // The soft-delete UPDATE also NULLs the child forgettable index column.
        assert!(out.contains(
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
        let out = output(input);
        assert!(out.contains(
            "UPDATE account_holders SET deleted = TRUE WHERE account_id = $1 AND deleted = FALSE"
        ));
        assert!(!out.contains("DELETE FROM"));
        assert!(!out.contains("= NULL"));
    }

    #[test]
    fn non_soft_delete_repo_gets_no_cascade_delete_impl() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(
                entity = "OrderItem",
                columns(order_id(ty = "OrderId", update(persist = false), parent))
            )]
            struct OrderItems {
                pool: sqlx::PgPool,
            }
        };
        let out = output(input);
        assert!(!out.contains("CascadeDeleteNested"));
    }
}
