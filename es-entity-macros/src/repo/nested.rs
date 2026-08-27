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
        let delete_fn_name = self.field.delete_nested_fn_name();
        let find_include_deleted_fn_name = self.field.find_nested_include_deleted_fn_name();
        let pending_work_fn_name = self.field.pending_nested_work_fn_name();

        tokens.append_all(quote! {
            // In-memory check for un-persisted work in this nested field's
            // subtree, used by the root's update path to decide whether the
            // aggregate version must be bumped.
            //
            // Recurses through the child repo so a dirty grandchild under a
            // clean child is still seen: a depth-1 check would let a
            // grandchild-only mutation reach the database without bumping the
            // root version.
            fn #pending_work_fn_name<P>(entities: &mut [&mut P]) -> bool
                where
                    P: es_entity::Parent<<#nested_repo_ty as es_entity::EsRepo>::Entity>,
            {
                for entity in entities.iter_mut() {
                    if !entity.new_children_mut().is_empty() {
                        return true;
                    }
                    let mut persisted: Vec<_> = entity.iter_persisted_children_mut().collect();
                    if persisted.iter().any(|child| child.events().any_new()) {
                        return true;
                    }
                    if <#nested_repo_ty as es_entity::EsRepo>::has_pending_nested_work(&mut persisted) {
                        return true;
                    }
                }
                false
            }

            // Batches every parent's new children into a single
            // `create_all_in_op` call on the child repo — one statement for
            // the whole parent batch, not one per parent — then redistributes
            // the hydrated children back to their owning parent by count.
            //
            // Takes `&mut [&mut P]` rather than `&mut [P]` so a caller that
            // already holds scattered `&mut P` borrows (this same fn one
            // level up the nesting, recursing into grandchildren) can pass
            // them straight through without needing contiguous storage.
            async fn #create_fn_name<OP, P>(&self, op: &mut OP, entities: &mut [&mut P]) -> Result<(), <#nested_repo_ty as es_entity::EsRepo>::CreateError>
                where
                    P: es_entity::Parent<<#nested_repo_ty as EsRepo>::Entity>,
                    OP: es_entity::AtomicOperation
            {
                let counts: Vec<usize> = entities
                    .iter_mut()
                    .map(|entity| entity.new_children_mut().len())
                    .collect();
                if counts.iter().all(|n| *n == 0) {
                    return Ok(());
                }

                let new_children: Vec<_> = entities
                    .iter_mut()
                    .flat_map(|entity| entity.new_children_mut().drain(..))
                    .collect();

                let mut children = self.#repo_field.create_all_in_op(op, new_children).await?.into_iter();
                for (entity, n) in entities.iter_mut().zip(counts) {
                    entity.inject_children(children.by_ref().take(n));
                }
                Ok(())
            }

            // Gathers every parent's already-persisted children into a single
            // `update_all_mut_in_op` call on the child repo — one statement
            // for the whole parent batch, not one per child per parent — then
            // batches new children via `#create_fn_name`.
            async fn #update_fn_name<OP, P>(&self, op: &mut OP, entities: &mut [&mut P]) -> Result<(), #parent_modify_error>
                where
                    P: es_entity::Parent<<#nested_repo_ty as EsRepo>::Entity>,
                    OP: es_entity::AtomicOperation
            {
                let persisted: Vec<_> = entities
                    .iter_mut()
                    .flat_map(|entity| entity.iter_persisted_children_mut())
                    .collect();
                if !persisted.is_empty() {
                    self.#repo_field.update_all_mut_in_op(op, persisted).await?;
                }
                self.#create_fn_name(op, entities).await?;
                Ok(())
            }

            async fn #find_fn_name<OP, P, __EsErr>(op: &mut OP, entities: &mut [P]) -> Result<(), __EsErr>
                where
                    OP: es_entity::AtomicOperation,
                    P: es_entity::Parent<<#nested_repo_ty as es_entity::EsRepo>::Entity> + es_entity::EsEntity,
                    #nested_repo_ty: es_entity::PopulateNested<<<P as es_entity::EsEntity>::Event as es_entity::EsEvent>::EntityId>,
                    __EsErr: From<sqlx::Error> + From<es_entity::EntityHydrationError> + Send,
            {
                let lookup = entities.iter_mut().map(|e| (e.events().entity_id.clone(), e)).collect();
                <#nested_repo_ty>::populate_in_op::<_, _, __EsErr>(op, lookup).await?;
                Ok(())
            }

            async fn #find_include_deleted_fn_name<OP, P, __EsErr>(op: &mut OP, entities: &mut [P]) -> Result<(), __EsErr>
                where
                    OP: es_entity::AtomicOperation,
                    P: es_entity::Parent<<#nested_repo_ty as es_entity::EsRepo>::Entity> + es_entity::EsEntity,
                    #nested_repo_ty: es_entity::PopulateNested<<<P as es_entity::EsEntity>::Event as es_entity::EsEvent>::EntityId>,
                    __EsErr: From<sqlx::Error> + From<es_entity::EntityHydrationError> + Send,
            {
                let lookup = entities.iter_mut().map(|e| (e.events().entity_id.clone(), e)).collect();
                <#nested_repo_ty>::populate_in_op_include_deleted::<_, _, __EsErr>(op, lookup).await?;
                Ok(())
            }

            async fn #delete_fn_name<OP, P, __EsErr>(op: &mut OP, entity: &P) -> Result<(), __EsErr>
                where
                    OP: es_entity::AtomicOperation,
                    P: es_entity::EsEntity,
                    #nested_repo_ty: es_entity::CascadeDeleteNested<<<P as es_entity::EsEntity>::Event as es_entity::EsEvent>::EntityId>,
                    __EsErr: From<sqlx::Error> + Send,
            {
                <#nested_repo_ty>::cascade_delete_in_op::<_, __EsErr>(op, &entity.events().entity_id).await?;
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
            entity: None,
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
            fn has_pending_nested_users_work<P>(entities: &mut [&mut P]) -> bool
                where
                    P: es_entity::Parent<<UserRepo as es_entity::EsRepo>::Entity>,
            {
                for entity in entities.iter_mut() {
                    if !entity.new_children_mut().is_empty() {
                        return true;
                    }
                    let mut persisted: Vec<_> = entity.iter_persisted_children_mut().collect();
                    if persisted.iter().any(|child| child.events().any_new()) {
                        return true;
                    }
                    if <UserRepo as es_entity::EsRepo>::has_pending_nested_work(&mut persisted) {
                        return true;
                    }
                }
                false
            }

            async fn create_nested_users_in_op<OP, P>(&self, op: &mut OP, entities: &mut [&mut P]) -> Result<(), <UserRepo as es_entity::EsRepo>::CreateError>
                where
                    P: es_entity::Parent<<UserRepo as EsRepo>::Entity>,
                    OP: es_entity::AtomicOperation
            {
                let counts: Vec<usize> = entities
                    .iter_mut()
                    .map(|entity| entity.new_children_mut().len())
                    .collect();
                if counts.iter().all(|n| *n == 0) {
                    return Ok(());
                }

                let new_children: Vec<_> = entities
                    .iter_mut()
                    .flat_map(|entity| entity.new_children_mut().drain(..))
                    .collect();

                let mut children = self.users.create_all_in_op(op, new_children).await?.into_iter();
                for (entity, n) in entities.iter_mut().zip(counts) {
                    entity.inject_children(children.by_ref().take(n));
                }
                Ok(())
            }

            async fn update_nested_users_in_op<OP, P>(&self, op: &mut OP, entities: &mut [&mut P]) -> Result<(), ParentModifyError>
                where
                    P: es_entity::Parent<<UserRepo as EsRepo>::Entity>,
                    OP: es_entity::AtomicOperation
            {
                let persisted: Vec<_> = entities
                    .iter_mut()
                    .flat_map(|entity| entity.iter_persisted_children_mut())
                    .collect();
                if !persisted.is_empty() {
                    self.users.update_all_mut_in_op(op, persisted).await?;
                }
                self.create_nested_users_in_op(op, entities).await?;
                Ok(())
            }

            async fn find_nested_users_in_op<OP, P, __EsErr>(op: &mut OP, entities: &mut [P]) -> Result<(), __EsErr>
                where
                    OP: es_entity::AtomicOperation,
                    P: es_entity::Parent<<UserRepo as es_entity::EsRepo>::Entity> + es_entity::EsEntity,
                    UserRepo: es_entity::PopulateNested<<<P as es_entity::EsEntity>::Event as es_entity::EsEvent>::EntityId>,
                    __EsErr: From<sqlx::Error> + From<es_entity::EntityHydrationError> + Send,
            {
                let lookup = entities.iter_mut().map(|e| (e.events().entity_id.clone(), e)).collect();
                <UserRepo>::populate_in_op::<_, _, __EsErr>(op, lookup).await?;
                Ok(())
            }

            async fn find_nested_users_include_deleted_in_op<OP, P, __EsErr>(op: &mut OP, entities: &mut [P]) -> Result<(), __EsErr>
                where
                    OP: es_entity::AtomicOperation,
                    P: es_entity::Parent<<UserRepo as es_entity::EsRepo>::Entity> + es_entity::EsEntity,
                    UserRepo: es_entity::PopulateNested<<<P as es_entity::EsEntity>::Event as es_entity::EsEvent>::EntityId>,
                    __EsErr: From<sqlx::Error> + From<es_entity::EntityHydrationError> + Send,
            {
                let lookup = entities.iter_mut().map(|e| (e.events().entity_id.clone(), e)).collect();
                <UserRepo>::populate_in_op_include_deleted::<_, _, __EsErr>(op, lookup).await?;
                Ok(())
            }

            async fn delete_nested_users_in_op<OP, P, __EsErr>(op: &mut OP, entity: &P) -> Result<(), __EsErr>
                where
                    OP: es_entity::AtomicOperation,
                    P: es_entity::EsEntity,
                    UserRepo: es_entity::CascadeDeleteNested<<<P as es_entity::EsEntity>::Event as es_entity::EsEvent>::EntityId>,
                    __EsErr: From<sqlx::Error> + Send,
            {
                <UserRepo>::cascade_delete_in_op::<_, __EsErr>(op, &entity.events().entity_id).await?;
                Ok(())
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
