//! Traits to orchestrate and maintain the event-sourcing pattern

use serde::{Serialize, de::DeserializeOwned};

use super::{error::EsEntityError, events::EntityEvents, operation::AtomicOperation};

pub trait EsEvent: DeserializeOwned + Serialize + Send + Sync {
    type EntityId: Clone
        + PartialEq
        + sqlx::Type<sqlx::Postgres>
        + Eq
        + std::hash::Hash
        + Send
        + Sync;
}

pub trait IntoEvents<E: EsEvent> {
    fn into_events(self) -> EntityEvents<E>;
}

pub trait TryFromEvents<E: EsEvent> {
    fn try_from_events(events: EntityEvents<E>) -> Result<Self, EsEntityError>
    where
        Self: Sized;
}

pub trait EsEntity: TryFromEvents<Self::Event> {
    type Event: EsEvent;
    type New: IntoEvents<Self::Event>;

    fn events(&self) -> &EntityEvents<Self::Event>;
    fn last_persisted(&self, n: usize) -> crate::events::LastPersisted<Self::Event> {
        self.events().last_persisted(n)
    }

    fn events_mut(&mut self) -> &mut EntityEvents<Self::Event>;
}

pub trait EsRepo {
    type Entity: EsEntity;
    type Err: From<EsEntityError> + From<sqlx::Error>;
    type EsQueryFlavor;

    fn load_all_nested_in_op<OP>(
        op: &mut OP,
        entities: &mut [Self::Entity],
    ) -> impl Future<Output = Result<(), Self::Err>> + Send
    where
        OP: AtomicOperation;
}

pub trait RetryableInto<T>: Into<T> + Copy + std::fmt::Debug {}
impl<T, O> RetryableInto<O> for T where T: Into<O> + Copy + std::fmt::Debug {}
