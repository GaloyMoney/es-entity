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

    pub fn new_entities_mut(&mut self) -> &mut Vec<<T as EsEntity>::New> {
        &mut self.new_entities
    }

    pub fn entities(&self) -> &HashMap<<<T as EsEntity>::Event as EsEvent>::EntityId, T> {
        &self.entities
    }

    pub fn entities_mut(
        &mut self,
    ) -> &mut HashMap<<<T as EsEntity>::Event as EsEvent>::EntityId, T> {
        &mut self.entities
    }

    pub fn extend_entities(&mut self, entities: impl IntoIterator<Item = T>) {
        self.entities.extend(
            entities
                .into_iter()
                .map(|entity| (entity.events().entity_id.clone(), entity)),
        );
    }
}

pub trait PopulateNested<C>: EsRepo {
    fn populate_in_op<OP>(
        op: &mut OP,
        lookup: std::collections::HashMap<C, &mut Nested<<Self as EsRepo>::Entity>>,
    ) -> impl Future<Output = Result<(), <Self as EsRepo>::Err>> + Send
    where
        OP: AtomicOperation;
}

pub trait Parent<T: EsEntity> {
    fn nested(&self) -> &Nested<T>;
    fn nested_mut(&mut self) -> &mut Nested<T>;
}
