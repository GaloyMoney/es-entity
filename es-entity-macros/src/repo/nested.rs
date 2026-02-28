use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::{RepoField, RepositoryOptions};

pub struct Nested<'a> {
    field: &'a RepoField,
    parent_modify_error: syn::Ident,
}

impl<'a> Nested<'a> {
    pub fn new(field: &'a RepoField, opts: &'a RepositoryOptions) -> Nested<'a> {
        Nested {
            field,
            parent_modify_error: opts.modify_error(),
        }
    }
}

impl ToTokens for Nested<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let parent_modify_error = &self.parent_modify_error;
        let repo_field = self.field.ident();

        let nested_repo_ty = &self.field.ty;
        let create_fn_name = self.field.create_nested_fn_name();
        let update_fn_name = self.field.update_nested_fn_name();
        let find_fn_name = self.field.find_nested_fn_name();

        tokens.append_all(quote! {
            async fn #create_fn_name<OP, P>(&self, op: &mut OP, entity: &mut P) -> Result<(), <#nested_repo_ty as es_entity::EsRepo>::CreateError>
                where
                    P: es_entity::Parent<<#nested_repo_ty as EsRepo>::Entity>,
                    OP: es_entity::AtomicOperation
            {
                let new_children = entity.new_children_mut();
                if new_children.is_empty() {
                    return Ok(());
                }

                let new_children = new_children.drain(..).collect();
                let children = self.#repo_field.create_all_in_op(op, new_children).await?;
                entity.inject_children(children);
                Ok(())
            }

            async fn #update_fn_name<OP, P>(&self, op: &mut OP, entity: &mut P) -> Result<(), #parent_modify_error>
                where
                    P: es_entity::Parent<<#nested_repo_ty as EsRepo>::Entity>,
                    OP: es_entity::AtomicOperation
            {
                for entity in entity.iter_persisted_children_mut() {
                    self.#repo_field.update_in_op(op, entity).await?;
                }
                self.#create_fn_name(op, entity).await?;
                Ok(())
            }

            async fn #find_fn_name<OP, P, E>(op: &mut OP, entities: &mut [P]) -> Result<(), E>
                where
                    OP: es_entity::AtomicOperation,
                    P: es_entity::Parent<<#nested_repo_ty as es_entity::EsRepo>::Entity> + es_entity::EsEntity,
                    #nested_repo_ty: es_entity::PopulateNested<<<P as es_entity::EsEntity>::Event as es_entity::EsEvent>::EntityId>,
                    E: From<sqlx::Error> + From<es_entity::EntityHydrationError> + Send,
            {
                let lookup = entities.iter_mut().map(|e| (e.events().entity_id.clone(), e)).collect();
                <#nested_repo_ty>::populate_in_op::<_, _, E>(op, lookup).await?;
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
            clock: false,
        };

        let cursor = Nested {
            field: &field,
            parent_modify_error: syn::Ident::new(
                "ParentModifyError",
                proc_macro2::Span::call_site(),
            ),
        };

        let mut tokens = TokenStream::new();
        cursor.to_tokens(&mut tokens);

        let expected = quote! {
            async fn create_nested_users_in_op<OP, P>(&self, op: &mut OP, entity: &mut P) -> Result<(), <UserRepo as es_entity::EsRepo>::CreateError>
                where
                    P: es_entity::Parent<<UserRepo as EsRepo>::Entity>,
                    OP: es_entity::AtomicOperation
            {
                let new_children = entity.new_children_mut();
                if new_children.is_empty() {
                    return Ok(());
                }

                let new_children = new_children.drain(..).collect();
                let children = self.users.create_all_in_op(op, new_children).await?;
                entity.inject_children(children);
                Ok(())
            }

            async fn update_nested_users_in_op<OP, P>(&self, op: &mut OP, entity: &mut P) -> Result<(), ParentModifyError>
                where
                    P: es_entity::Parent<<UserRepo as EsRepo>::Entity>,
                    OP: es_entity::AtomicOperation
            {
                for entity in entity.iter_persisted_children_mut() {
                    self.users.update_in_op(op, entity).await?;
                }
                self.create_nested_users_in_op(op, entity).await?;
                Ok(())
            }

            async fn find_nested_users_in_op<OP, P, E>(op: &mut OP, entities: &mut [P]) -> Result<(), E>
                where
                    OP: es_entity::AtomicOperation,
                    P: es_entity::Parent<<UserRepo as es_entity::EsRepo>::Entity> + es_entity::EsEntity,
                    UserRepo: es_entity::PopulateNested<<<P as es_entity::EsEntity>::Event as es_entity::EsEvent>::EntityId>,
                    E: From<sqlx::Error> + From<es_entity::EntityHydrationError> + Send,
            {
                let lookup = entities.iter_mut().map(|e| (e.events().entity_id.clone(), e)).collect();
                <UserRepo>::populate_in_op::<_, _, E>(op, lookup).await?;
                Ok(())
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
