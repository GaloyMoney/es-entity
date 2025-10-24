use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::*;

pub struct DeleteFn<'a> {
    error: &'a syn::Type,
    entity: &'a syn::Ident,
    table_name: &'a str,
    columns: &'a Columns,
    delete_option: &'a DeleteOption,
    additional_op_constraint: proc_macro2::TokenStream,
}

impl<'a> DeleteFn<'a> {
    pub fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            entity: opts.entity(),
            error: opts.err(),
            columns: &opts.columns,
            table_name: opts.table_name(),
            delete_option: &opts.delete,
            additional_op_constraint: opts.additional_op_constraint(),
        }
    }
}

impl ToTokens for DeleteFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        if matches!(self.delete_option, DeleteOption::No) {
            return;
        }

        let entity = self.entity;
        let error = self.error;
        let additional_op_constraint = &self.additional_op_constraint;

        let assignments = self
            .columns
            .variable_assignments_for_update(syn::parse_quote! { entity });
        let column_updates = self.columns.sql_updates();
        let query = format!(
            "UPDATE {} SET {}{}deleted = TRUE WHERE id = $1",
            self.table_name,
            column_updates,
            if column_updates.is_empty() { "" } else { ", " }
        );
        let args = self.columns.update_query_args();

        #[cfg(feature = "instrument")]
        let (instrument_attr, record_id) = {
            let entity_name = entity.to_string();
            (
                quote! {
                    #[tracing::instrument(skip_all, fields(entity = #entity_name, id = tracing::field::Empty), err(level = "warn"))]
                },
                quote! {
                    tracing::Span::current().record("id", tracing::field::debug(&entity.id));
                },
            )
        };
        #[cfg(not(feature = "instrument"))]
        let (instrument_attr, record_id) = (quote! {}, quote! {});

        tokens.append_all(quote! {
            pub async fn delete(
                &self,
                entity: #entity
            ) -> Result<(), #error> {
                let mut op = self.begin_op().await?;
                let res = self.delete_in_op(&mut op, entity).await?;
                op.commit().await?;
                Ok(res)
            }

            #instrument_attr
            pub async fn delete_in_op<OP>(&self,
                op: &mut OP,
                mut entity: #entity
            ) -> Result<(), #error>
            where
                OP: es_entity::AtomicOperation
                #additional_op_constraint
            {
                #assignments
                #record_id

                sqlx::query!(
                    #query,
                    #(#args),*
                )
                    .execute(op.as_executor())
                    .await?;

                let new_events = {
                    let events = Self::extract_events(&mut entity);
                    events.any_new()
                };

                if new_events {
                    let n_events = {
                        let events = Self::extract_events(&mut entity);
                        self.persist_events(op, events).await?
                    };

                    self.execute_post_persist_hook(op, &entity, entity.events().last_persisted(n_events)).await?;
                }

                Ok(())
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
    fn delete_fn() {
        let id = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let error = syn::parse_str("es_entity::EsRepoError").unwrap();
        let mut columns = Columns::default();
        columns.set_id_column(&id);

        let delete_fn = DeleteFn {
            entity: &entity,
            error: &error,
            table_name: "entities",
            columns: &columns,
            delete_option: &DeleteOption::Soft,
            additional_op_constraint: quote! {},
        };

        let mut tokens = TokenStream::new();
        delete_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn delete(
                &self,
                entity: Entity
            ) -> Result<(), #error> {
                let mut op = self.begin_op().await?;
                let res = self.delete_in_op(&mut op, entity).await?;
                op.commit().await?;
                Ok(res)
            }

            pub async fn delete_in_op<OP>(
                &self,
                op: &mut OP,
                mut entity: Entity
            ) -> Result<(), es_entity::EsRepoError>
            where
                OP: es_entity::AtomicOperation
            {
                let id = &entity.id;

                sqlx::query!(
                    "UPDATE entities SET deleted = TRUE WHERE id = $1",
                    id as &EntityId
                )
                    .execute(op.as_executor())
                    .await?;

                let new_events = {
                    let events = Self::extract_events(&mut entity);
                    events.any_new()
                };

                if new_events {
                    let n_events = {
                        let events = Self::extract_events(&mut entity);
                        self.persist_events(op, events).await?
                    };

                    self.execute_post_persist_hook(op, &entity, entity.events().last_persisted(n_events)).await?;
                }

                Ok(())
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn delete_fn_with_update_columns() {
        let id = syn::parse_str("EntityId").unwrap();
        let entity = Ident::new("Entity", Span::call_site());
        let error = syn::parse_str("es_entity::EsRepoError").unwrap();

        let columns = Columns::new(
            &id,
            [Column::new(
                Ident::new("name", Span::call_site()),
                syn::parse_str("String").unwrap(),
            )],
        );

        let delete_fn = DeleteFn {
            entity: &entity,
            error: &error,
            table_name: "entities",
            columns: &columns,
            delete_option: &DeleteOption::Soft,
            additional_op_constraint: quote! {},
        };

        let mut tokens = TokenStream::new();
        delete_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn delete(
                &self,
                entity: Entity
            ) -> Result<(), #error> {
                let mut op = self.begin_op().await?;
                let res = self.delete_in_op(&mut op, entity).await?;
                op.commit().await?;
                Ok(res)
            }

            pub async fn delete_in_op<OP>(
                &self,
                op: &mut OP,
                mut entity: Entity
            ) -> Result<(), es_entity::EsRepoError>
            where
                OP: es_entity::AtomicOperation
            {
                let id = &entity.id;
                let name = &entity.name;

                sqlx::query!(
                    "UPDATE entities SET name = $2, deleted = TRUE WHERE id = $1",
                    id as &EntityId,
                    name as &String
                )
                    .execute(op.as_executor())
                    .await?;

                let new_events = {
                    let events = Self::extract_events(&mut entity);
                    events.any_new()
                };

                if new_events {
                    let n_events = {
                        let events = Self::extract_events(&mut entity);
                        self.persist_events(op, events).await?
                    };

                    self.execute_post_persist_hook(op, &entity, entity.events().last_persisted(n_events)).await?;
                }

                Ok(())
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
