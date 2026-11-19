//! Handle operations for nested entities.

use crate::{db, operation::AtomicOperation, traits::*};

use std::{collections::HashMap, marker::PhantomData};

/// Delete mode of a nested field whose children are never removed (default).
pub struct NoDelete;

/// Delete mode of a nested field whose children can be soft-deleted.
///
/// Requires the child repo to have the soft delete option
/// (`#[es_repo(delete = "soft")]`).
pub struct SoftDelete;

pub struct Nested<T: EsEntity, D = NoDelete> {
    entities: HashMap<<<T as EsEntity>::Event as EsEvent>::EntityId, T>,
    new_entities: Vec<<T as EsEntity>::New>,
    removed: Vec<T>,
    _delete: PhantomData<fn() -> D>,
}

impl<T: EsEntity, D> Default for Nested<T, D> {
    fn default() -> Self {
        Self {
            entities: HashMap::new(),
            new_entities: Vec::new(),
            removed: Vec::new(),
            _delete: PhantomData,
        }
    }
}

impl<T: EsEntity> Nested<T, SoftDelete> {
    /// Remove a persisted child. The parent repo's `update` deletes it.
    ///
    /// Returns the removed child so a final event can still be pushed on it,
    /// or `None` if `id` is not a persisted child.
    pub fn delete_persisted(
        &mut self,
        id: &<<T as EsEntity>::Event as EsEvent>::EntityId,
    ) -> Option<&mut T> {
        let entity = self.entities.remove(id)?;
        self.removed.push(entity);
        self.removed.last_mut()
    }

    pub fn len_removed(&self) -> usize {
        self.removed.len()
    }

    pub fn iter_removed(&self) -> impl Iterator<Item = &T> {
        self.removed.iter()
    }
}

impl<T: EsEntity, D> Nested<T, D> {
    #[doc(hidden)]
    pub fn take_removed(&mut self) -> Vec<T> {
        std::mem::take(&mut self.removed)
    }

    pub fn add_new(&mut self, new: <T as EsEntity>::New) -> &<T as EsEntity>::New {
        let len = self.new_entities.len();
        self.new_entities.push(new);
        &self.new_entities[len]
    }

    pub fn len_new(&self) -> usize {
        self.new_entities.len()
    }

    pub fn find_new<P>(&self, predicate: P) -> Option<&<T as EsEntity>::New>
    where
        P: FnMut(&&<T as EsEntity>::New) -> bool,
    {
        self.new_entities.iter().find(predicate)
    }

    pub fn find_map_new<B, F>(&self, f: F) -> Option<B>
    where
        F: FnMut(&<T as EsEntity>::New) -> Option<B>,
    {
        self.new_entities.iter().find_map(f)
    }

    pub fn new_entities_mut(&mut self) -> &mut Vec<<T as EsEntity>::New> {
        &mut self.new_entities
    }

    pub fn get_persisted(&self, id: &<<T as EsEntity>::Event as EsEvent>::EntityId) -> Option<&T> {
        self.entities.get(id)
    }

    pub fn get_persisted_mut(
        &mut self,
        id: &<<T as EsEntity>::Event as EsEvent>::EntityId,
    ) -> Option<&mut T> {
        self.entities.get_mut(id)
    }

    pub fn find_persisted<P>(&self, predicate: P) -> Option<&T>
    where
        P: FnMut(&&T) -> bool,
    {
        self.entities.values().find(predicate)
    }

    pub fn find_map_persisted<B, F>(&self, f: F) -> Option<B>
    where
        F: FnMut(&T) -> Option<B>,
    {
        self.entities.values().find_map(f)
    }

    pub fn find_persisted_mut<P>(&mut self, predicate: P) -> Option<&mut T>
    where
        P: FnMut(&&mut T) -> bool,
    {
        self.entities.values_mut().find(predicate)
    }

    pub fn len_persisted(&self) -> usize {
        self.entities.len()
    }

    pub fn iter_persisted(
        &self,
    ) -> std::collections::hash_map::Values<'_, <<T as EsEntity>::Event as EsEvent>::EntityId, T>
    {
        self.entities.values()
    }

    pub fn iter_persisted_mut(
        &mut self,
    ) -> std::collections::hash_map::ValuesMut<'_, <<T as EsEntity>::Event as EsEvent>::EntityId, T>
    {
        self.entities.values_mut()
    }

    pub fn load(&mut self, entities: impl IntoIterator<Item = T>) {
        self.entities.extend(
            entities
                .into_iter()
                .map(|entity| (entity.events().entity_id.clone(), entity)),
        );
    }
}

pub trait HydrateNested<ID>: EsRepo {
    fn hydrate_in_op<P>(
        rows_by_tag: &mut HashMap<i32, Vec<db::Row>>,
        tag_cursor: &mut i32,
        lookup: HashMap<ID, &mut P>,
    ) -> Result<(), crate::RepoFault>
    where
        P: Parent<<Self as EsRepo>::Entity>;
}

/// Trait for cascade soft-deleting child entities when a parent is deleted.
///
/// Generated automatically for nested repositories that have both a `parent` column
/// and `delete = "soft"` configured.
pub trait CascadeDeleteNested<ID>: EsRepo {
    fn cascade_delete_in_op<OP>(
        op: &mut OP,
        parent_id: &ID,
    ) -> impl Future<Output = Result<(), crate::RepoFault>> + Send
    where
        OP: AtomicOperation + ?Sized;

    /// Batch form of [`cascade_delete_in_op`](Self::cascade_delete_in_op):
    /// one set of statements for every id in `parent_ids`.
    fn cascade_delete_all_in_op<OP>(
        op: &mut OP,
        parent_ids: &[ID],
    ) -> impl Future<Output = Result<(), crate::RepoFault>> + Send
    where
        OP: AtomicOperation + ?Sized;
}

/// Extracts the entity and delete mode from a `Nested<T, D>` field type.
#[doc(hidden)]
pub trait NestedField {
    type Entity: EsEntity;
    type Delete;
}

impl<T: EsEntity, D> NestedField for Nested<T, D> {
    type Entity = T;
    type Delete = D;
}

/// Emitted by `EsRepo` for a repo with a `parent` column and soft delete.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot delete nested children",
    note = "set the soft delete option on the child repo's `#[es_repo(...)]`, or use plain `Nested<T>` on the parent entity"
)]
pub trait DeleteNested: EsRepo {
    type Error;
    fn delete_removed_in_op<OP>(
        &self,
        op: &mut OP,
        entities: Vec<<Self as EsRepo>::Entity>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send
    where
        OP: AtomicOperation + ?Sized;
}

/// Implemented for the two delete modes. The single place where the mode on the
/// parent entity is checked against the child repo `R`.
#[doc(hidden)]
pub trait FlushRemoved<R: EsRepo, E> {
    fn flush<OP>(
        repo: &R,
        op: &mut OP,
        removed: Vec<<R as EsRepo>::Entity>,
    ) -> impl Future<Output = Result<(), E>> + Send
    where
        OP: AtomicOperation + ?Sized;
}

impl<R: EsRepo + Sync, E> FlushRemoved<R, E> for NoDelete {
    async fn flush<OP>(
        _repo: &R,
        _op: &mut OP,
        removed: Vec<<R as EsRepo>::Entity>,
    ) -> Result<(), E>
    where
        OP: AtomicOperation + ?Sized,
    {
        debug_assert!(removed.is_empty());
        Ok(())
    }
}

impl<R, E> FlushRemoved<R, E> for SoftDelete
where
    R: DeleteNested + Sync,
    E: From<<R as DeleteNested>::Error>,
{
    async fn flush<OP>(repo: &R, op: &mut OP, removed: Vec<<R as EsRepo>::Entity>) -> Result<(), E>
    where
        OP: AtomicOperation + ?Sized,
    {
        if removed.is_empty() {
            return Ok(());
        }
        repo.delete_removed_in_op(op, removed)
            .await
            .map_err(E::from)
    }
}

/// Checks a parent entity's delete mode against the child repo `R`: the
/// bound the generated nested update puts on its parent type. Blanket
/// implemented for every [`Parent`] whose `Delete` mode is a [`FlushRemoved`]
/// for `R`.
#[doc(hidden)]
pub trait FlushNested<R: EsRepo, E> {
    fn flush_removed<OP>(
        repo: &R,
        op: &mut OP,
        removed: Vec<<R as EsRepo>::Entity>,
    ) -> impl Future<Output = Result<(), E>> + Send
    where
        OP: AtomicOperation + ?Sized;
}

impl<P, R, E> FlushNested<R, E> for P
where
    R: EsRepo,
    P: Parent<<R as EsRepo>::Entity>,
    <P as Parent<<R as EsRepo>::Entity>>::Delete: FlushRemoved<R, E>,
{
    fn flush_removed<OP>(
        repo: &R,
        op: &mut OP,
        removed: Vec<<R as EsRepo>::Entity>,
    ) -> impl Future<Output = Result<(), E>> + Send
    where
        OP: AtomicOperation + ?Sized,
    {
        <<P as Parent<<R as EsRepo>::Entity>>::Delete as FlushRemoved<R, E>>::flush(
            repo, op, removed,
        )
    }
}

/// Trait that entities implement for every field marked `#[es_entity(nested)]`
///
/// Will be auto-implemented when [`#[derive(EsEntity)]`](`EsEntity`) is used.
pub trait Parent<T: EsEntity>: Send {
    /// `NoDelete` or `SoftDelete`, taken from the nested field's type.
    type Delete;
    /// Drain children removed with `delete_persisted` so the repo can delete them.
    fn take_removed_children(&mut self) -> Vec<T>;
    /// Access new child entities to persist them.
    fn new_children_mut(&mut self) -> &mut Vec<<T as EsEntity>::New>;
    /// Access existing children to update them incase they were mutated.
    fn iter_persisted_children_mut(
        &mut self,
    ) -> std::collections::hash_map::ValuesMut<'_, <<T as EsEntity>::Event as EsEvent>::EntityId, T>;
    /// Inject hydrated children while loading the parent.
    fn inject_children(&mut self, entities: impl IntoIterator<Item = T>);
}
