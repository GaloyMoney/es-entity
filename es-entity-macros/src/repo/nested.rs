use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::{RepoField, RepositoryOptions};

pub struct Nested<'a> {
    field: &'a RepoField,
    error: &'a syn::Type,
    additional_op_constraint: proc_macro2::TokenStream,
}

impl<'a> Nested<'a> {
    pub fn new(field: &'a RepoField, opts: &'a RepositoryOptions) -> Nested<'a> {
        Nested {
            field,
            error: opts.err(),
            additional_op_constraint: opts.additional_op_constraint(),
        }
    }
}

impl ToTokens for Nested<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let error = self.error;
        let repo_field = self.field.ident();
        let additional_op_constraint = &self.additional_op_constraint;

        let nested_repo_ty = &self.field.ty;
        let create_fn_name = self.field.create_nested_fn_name();
        let update_fn_name = self.field.update_nested_fn_name();
        let find_fn_name = self.field.find_nested_fn_name();

        tokens.append_all(quote! {
            async fn #create_fn_name<OP, P>(&self, op: &mut OP, entity: &mut P) -> Result<(), #error>
                where
                    P: es_entity::Parent<<#nested_repo_ty as EsRepo>::Entity>,
                    OP: es_entity::AtomicOperation,
                    #additional_op_constraint
            {
                let nested = entity.nested_mut();
                if nested.new_entities_mut().is_empty() {
                    return Ok(());
                }

                let new_entities = nested.new_entities_mut().drain(..).collect();
                let entities = self.#repo_field.create_all_in_op(&mut *op, new_entities).await?;
                nested.extend_entities(entities);
                Ok(())
            }

            async fn #update_fn_name<OP, P>(&self, op: &mut OP, entity: &mut P) -> Result<(), #error>
                where
                    P: es_entity::Parent<<#nested_repo_ty as EsRepo>::Entity>,
                    OP: es_entity::AtomicOperation,
                    #additional_op_constraint
            {
                let entities = entity.nested_mut().entities_mut();
                for entity in entities.values_mut() {
                    self.#repo_field.update_in_op(&mut *op, entity).await?;
                }
                self.#create_fn_name(&mut *op, entity).await?;
                Ok(())
            }

            async fn #find_fn_name<OP, P>(op: &mut OP, entities: &mut [P]) -> Result<(), #error>
                where
                    OP: es_entity::AtomicOperation,
                    P: es_entity::Parent<<#nested_repo_ty as es_entity::EsRepo>::Entity> + es_entity::EsEntity,
                    #nested_repo_ty: es_entity::PopulateNested<<<P as es_entity::EsEntity>::Event as es_entity::EsEvent>::EntityId>,
                    #error: From<<#nested_repo_ty as es_entity::EsRepo>::Err>
            {
                let lookup = entities.iter_mut().map(|e| (e.events().entity_id.clone(), e.nested_mut())).collect();
                #nested_repo_ty::populate_in_op(&mut *op, lookup).await?;
                Ok(())
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proc_macro2::Span;
    use syn::{Ident, parse_quote};

    #[test]
    fn nested() {
        let field = RepoField {
            ident: Some(Ident::new("users", Span::call_site())),
            ty: parse_quote! { UserRepo },
            nested: true,
            pool: false,
        };
        let error = syn::parse_str("es_entity::EsRepoError").unwrap();

        let cursor = Nested {
            error: &error,
            field: &field,
            additional_op_constraint: quote! {},
        };

        let mut tokens = TokenStream::new();
        cursor.to_tokens(&mut tokens);

        let expected = quote! {
            async fn create_nested_users_in_op<OP, P>(&self, op: &mut OP, entity: &mut P) -> Result<(), es_entity::EsRepoError>
                where
                    P: es_entity::Parent<<UserRepo as EsRepo>::Entity>,
                    OP: es_entity::AtomicOperation,
            {
                let nested = entity.nested_mut();
                if nested.new_entities_mut().is_empty() {
                    return Ok(());
                }

                let new_entities = nested.new_entities_mut().drain(..).collect();
                let entities = self.users.create_all_in_op(&mut *op, new_entities).await?;
                nested.extend_entities(entities);
                Ok(())
            }

            async fn update_nested_users_in_op<OP, P>(&self, op: &mut OP, entity: &mut P) -> Result<(), es_entity::EsRepoError>
                where
                    P: es_entity::Parent<<UserRepo as EsRepo>::Entity>,
                    OP: es_entity::AtomicOperation,
            {
                let entities = entity.nested_mut().entities_mut();
                for entity in entities.values_mut() {
                    self.users.update_in_op(&mut *op, entity).await?;
                }
                self.create_nested_users_in_op(&mut *op, entity).await?;
                Ok(())
            }

            async fn find_nested_users_in_op<OP, P>(op: &mut OP, entities: &mut [P]) -> Result<(), es_entity::EsRepoError>
                where
                    OP: es_entity::AtomicOperation,
                    P: es_entity::Parent<<UserRepo as es_entity::EsRepo>::Entity> + es_entity::EsEntity,
                    UserRepo: es_entity::PopulateNested<<<P as es_entity::EsEntity>::Event as es_entity::EsEvent>::EntityId>,
                    es_entity::EsRepoError: From<<UserRepo as es_entity::EsRepo>::Err>
            {
                let lookup = entities.iter_mut().map(|e| (e.events().entity_id.clone(), e.nested_mut())).collect();
                UserRepo::populate_in_op(&mut *op, lookup).await?;
                Ok(())
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
