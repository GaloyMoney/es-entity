use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::*;

pub struct ForgetFn<'a> {
    id: &'a syn::Ident,
    entity: &'a syn::Ident,
    event: &'a syn::Ident,
    error: syn::Ident,
    table_name: &'a str,
    forgettable_table_name: &'a str,
    forgettable_columns: Vec<&'a syn::Ident>,
}

impl<'a> ForgetFn<'a> {
    pub fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            id: opts.id(),
            entity: opts.entity(),
            event: opts.event(),
            error: opts.forget_error(),
            table_name: opts.table_name(),
            forgettable_table_name: opts
                .forgettable_table_name()
                .expect("forgettable must be enabled"),
            forgettable_columns: opts.columns.forgettable_column_names(),
        }
    }
}

impl ToTokens for ForgetFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let id_type = &self.id;
        let entity_type = self.entity;
        let event_type = self.event;
        let error = &self.error;

        let query = format!(
            "DELETE FROM {} WHERE entity_id = $1",
            self.forgettable_table_name
        );

        // Also NULL any `Forgettable<..>` index columns so the materialised
        // lookup table stops exposing the forgotten value.
        let forget_columns = if self.forgettable_columns.is_empty() {
            quote! {}
        } else {
            let set_clause = self
                .forgettable_columns
                .iter()
                .map(|c| format!("{} = NULL", c))
                .collect::<Vec<_>>()
                .join(", ");
            let columns_query = format!(
                "UPDATE {} SET {} WHERE id = $1",
                self.table_name, set_clause
            );
            quote! {
                sqlx::query!(
                    #columns_query,
                    id as &#id_type
                )
                .execute(op.as_executor())
                .await?;
            }
        };

        tokens.append_all(quote! {
            pub async fn forget(
                &self,
                entity: &mut #entity_type
            ) -> Result<(), #error> {
                let mut op = self.begin_op().await?;
                self.forget_in_op(&mut op, entity).await?;
                op.commit().await?;
                Ok(())
            }

            pub async fn forget_in_op<OP>(
                &self,
                op: &mut OP,
                entity: &mut #entity_type
            ) -> Result<(), #error>
            where
                OP: es_entity::AtomicOperation
            {
                let id = &entity.id;
                sqlx::query!(
                    #query,
                    id as &#id_type
                )
                .execute(op.as_executor())
                .await?;
                #forget_columns
                let events = entity.events_mut().forget_and_take(
                    #event_type::forget_forgettable_payloads
                );
                *entity = es_entity::TryFromEvents::try_from_events(events)?;
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
    fn forget_fn() {
        let id = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let event = Ident::new("EntityEvent", Span::call_site());
        let error = Ident::new("EntityForgetError", Span::call_site());

        let forget_fn = ForgetFn {
            id: &id,
            entity: &entity,
            event: &event,
            error,
            table_name: "entities",
            forgettable_table_name: "entities_forgettable_payloads",
            forgettable_columns: Vec::new(),
        };

        let mut tokens = TokenStream::new();
        forget_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn forget(
                &self,
                entity: &mut Entity
            ) -> Result<(), EntityForgetError> {
                let mut op = self.begin_op().await?;
                self.forget_in_op(&mut op, entity).await?;
                op.commit().await?;
                Ok(())
            }

            pub async fn forget_in_op<OP>(
                &self,
                op: &mut OP,
                entity: &mut Entity
            ) -> Result<(), EntityForgetError>
            where
                OP: es_entity::AtomicOperation
            {
                let id = &entity.id;
                sqlx::query!(
                    "DELETE FROM entities_forgettable_payloads WHERE entity_id = $1",
                    id as &EntityId
                )
                .execute(op.as_executor())
                .await?;
                let events = entity.events_mut().forget_and_take(
                    EntityEvent::forget_forgettable_payloads
                );
                *entity = es_entity::TryFromEvents::try_from_events(events)?;
                Ok(())
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
