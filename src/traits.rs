use async_trait::async_trait;
use serde::{Serialize, de::DeserializeOwned};

use super::{error::EsEntityError, events::EntityEvents, nested::*};

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

pub trait Parent<T: EsEntity> {
    fn nested(&self) -> &Nested<T>;
    fn nested_mut(&mut self) -> &mut Nested<T>;
}

#[async_trait]
pub trait EsRepo {
    type Entity: EsEntity;
    type Err: From<EsEntityError> + From<sqlx::Error>;

    async fn load_all_nested_in_op<OP>(
        op: &mut OP,
        entities: &mut [Self::Entity],
    ) -> Result<(), Self::Err>
    where
        OP: for<'o> AtomicOperation<'o>;
}

#[async_trait]
pub trait PopulateNested<C>: EsRepo {
    async fn populate_in_op<OP>(
        op: &mut OP,
        lookup: std::collections::HashMap<C, &mut Nested<<Self as EsRepo>::Entity>>,
    ) -> Result<(), <Self as EsRepo>::Err>
    where
        OP: for<'o> AtomicOperation<'o>;
}

pub trait RetryableInto<T>: Into<T> + Copy + std::fmt::Debug {}
impl<T, O> RetryableInto<O> for T where T: Into<O> + Copy + std::fmt::Debug {}

pub trait AtomicOperation<'a>: Send {
    type Executor: sqlx::Executor<'a, Database = sqlx::Postgres>;

    fn now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        None
    }

    fn as_executor(&'a mut self) -> Self::Executor;
}

impl<'a, 't> AtomicOperation<'a> for sqlx::Transaction<'t, sqlx::Postgres> {
    type Executor = &'a mut sqlx::PgConnection;

    fn as_executor(&'a mut self) -> Self::Executor {
        &mut *self
    }
}

pub trait IntoExecutor<'a> {
    type Executor: sqlx::Executor<'a, Database = sqlx::Postgres>;

    fn into_executor(self) -> Self::Executor;
}

impl<'a, T> IntoExecutor<'a> for &'a mut T
where
    T: AtomicOperation<'a>,
{
    type Executor = T::Executor;

    fn into_executor(self) -> Self::Executor {
        self.as_executor()
    }
}

impl<'a> IntoExecutor<'a> for &sqlx::PgPool {
    type Executor = Self;

    fn into_executor(self) -> Self::Executor {
        self
    }
}

impl<'a> IntoExecutor<'a> for &'a mut sqlx::PgConnection {
    type Executor = Self;

    fn into_executor(self) -> Self::Executor {
        self
    }
}
