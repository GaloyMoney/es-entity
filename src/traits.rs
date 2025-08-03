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
        OP: AtomicOperation;
}

#[async_trait]
pub trait PopulateNested<C>: EsRepo {
    async fn populate_in_op<OP>(
        op: &mut OP,
        lookup: std::collections::HashMap<C, &mut Nested<<Self as EsRepo>::Entity>>,
    ) -> Result<(), <Self as EsRepo>::Err>
    where
        OP: AtomicOperation;
}

pub trait RetryableInto<T>: Into<T> + Copy + std::fmt::Debug {}
impl<T, O> RetryableInto<O> for T where T: Into<O> + Copy + std::fmt::Debug {}

pub trait AtomicOperation: Send {
    type Executor<'c>: sqlx::Executor<'c, Database = sqlx::Postgres>
    where
        Self: 'c;

    fn now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        None
    }

    fn as_executor<'a, 'c>(&'a mut self) -> Self::Executor<'c>
    where
        'a: 'c;
}

impl<'t> AtomicOperation for sqlx::Transaction<'t, sqlx::Postgres> {
    type Executor<'c>
        = &'c mut sqlx::PgConnection
    where
        Self: 'c;

    fn as_executor<'a, 'c>(&'a mut self) -> Self::Executor<'c>
    where
        'a: 'c,
    {
        &mut **self
    }
}

pub trait IntoExecutor<'c> {
    type Executor: sqlx::Executor<'c, Database = sqlx::Postgres>;

    fn into_executor(self) -> Self::Executor;
}

impl<'a, 'c, T> IntoExecutor<'c> for &'a mut T
where
    'a: 'c,
    T: AtomicOperation,
{
    type Executor = T::Executor<'c>;
    fn into_executor(self) -> Self::Executor {
        self.as_executor()
    }
}

impl<'c> IntoExecutor<'c> for &sqlx::PgPool {
    type Executor = Self;

    fn into_executor(self) -> Self::Executor {
        self
    }
}

impl<'c> IntoExecutor<'c> for &'c mut sqlx::PgConnection {
    type Executor = Self;

    fn into_executor(self) -> Self::Executor {
        self
    }
}
