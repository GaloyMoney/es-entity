//! Handle operations for nested entities.

use crate::{operation::AtomicOperation, traits::*};

use std::collections::HashMap;

pub struct Nested<T: EsEntity> {
    entities: HashMap<<<T as EsEntity>::Event as EsEvent>::EntityId, T>,
    new_entities: Vec<<T as EsEntity>::New>,
}

impl<T: EsEntity> Default for Nested<T> {
    fn default() -> Self {
        Self {
            entities: HashMap::new(),
            new_entities: Vec::new(),
        }
    }
}

impl<T: EsEntity> Nested<T> {
    pub fn add_new(&mut self, new: <T as EsEntity>::New) -> &<T as EsEntity>::New {
        let len = self.new_entities.len();
        self.new_entities.push(new);
        &self.new_entities[len]
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

    pub fn new_entities_mut(&mut self) -> &mut Vec<<T as EsEntity>::New> {
        &mut self.new_entities
    }

    pub fn load(&mut self, entities: impl IntoIterator<Item = T>) {
        self.entities.extend(
            entities
                .into_iter()
                .map(|entity| (entity.events().entity_id.clone(), entity)),
        );
    }
}

pub trait PopulateNested<ID>: EsRepo {
    fn populate_in_op<OP, P>(
        op: &mut OP,
        lookup: std::collections::HashMap<ID, &mut P>,
    ) -> impl Future<Output = Result<(), <Self as EsRepo>::Err>> + Send
    where
        OP: AtomicOperation,
        P: Parent<<Self as EsRepo>::Entity>;
}

/// Trait that entities implement for every field marked `#[es_entity(nested)]`
///
/// Will be auto-implemented when [`#[derive(EsEntity)]`](`EsEntity`) is used.
pub trait Parent<T: EsEntity>: Send {
    /// Access new child entities to persist them.
    fn new_children_mut(&mut self) -> &mut Vec<<T as EsEntity>::New>;
    /// Access existing children to update them incase they were mutated.
    fn iter_persisted_children_mut(
        &mut self,
    ) -> std::collections::hash_map::ValuesMut<'_, <<T as EsEntity>::Event as EsEvent>::EntityId, T>;
    /// Inject hydrated children while loading the parent.
    fn inject_children(&mut self, entities: impl IntoIterator<Item = T>);
}
