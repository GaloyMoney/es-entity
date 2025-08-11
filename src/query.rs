//! Handle query generation with helper methods for execution.

use crate::{
    events::{EntityEvents, GenericEvent},
    one_time_executor::IntoOneTimeExecutor,
    operation::AtomicOperation,
    traits::*,
};

/// Type-safe wrapper around the [`EsRepo`]-generated or user-written [sqlx] query with execution helpers.
///
/// Provides separate `fetch` implementations for query execution on nested and flat entities decided by the
/// marker types internally ( [`EsQueryFlavorFlat`] or [`EsQueryFlavorNested`] ), both of which
/// internally call [`fetch_all`][crate::OneTimeExecutor::fetch_all] and subsequently load the entities
/// from their events to return them.
pub struct EsQuery<'q, Repo, Flavor, F, A> {
    inner: sqlx::query::Map<'q, sqlx::Postgres, F, A>,
    _repo: std::marker::PhantomData<Repo>,
    _flavor: std::marker::PhantomData<Flavor>,
}

/// Marker type for query execution on flat entities (entities without nested relationships).
pub struct EsQueryFlavorFlat;

/// Marker type for query execution on nested entities (entities with nested relationships).
pub struct EsQueryFlavorNested;

impl<'q, Repo, Flavor, F, A> EsQuery<'q, Repo, Flavor, F, A>
where
    Repo: EsRepo,
    <<<Repo as EsRepo>::Entity as EsEntity>::Event as EsEvent>::EntityId: Unpin,
    F: FnMut(
            sqlx::postgres::PgRow,
        ) -> Result<
            GenericEvent<<<<Repo as EsRepo>::Entity as EsEntity>::Event as EsEvent>::EntityId>,
            sqlx::Error,
        > + Send,
    A: 'q + Send + sqlx::IntoArguments<'q, sqlx::Postgres>,
{
    /// Creates a new [`EsQuery`] wrapper around the provided sqlx query.
    pub fn new(query: sqlx::query::Map<'q, sqlx::Postgres, F, A>) -> Self {
        Self {
            inner: query,
            _repo: std::marker::PhantomData,
            _flavor: std::marker::PhantomData,
        }
    }

    async fn fetch_optional_inner(
        self,
        op: impl IntoOneTimeExecutor<'_>,
    ) -> Result<Option<<Repo as EsRepo>::Entity>, <Repo as EsRepo>::Err> {
        let executor = op.into_executor();
        let rows = executor.fetch_all(self.inner).await?;
        if rows.is_empty() {
            return Ok(None);
        }

        Ok(Some(EntityEvents::load_first(rows.into_iter())?))
    }

    async fn fetch_one_inner(
        self,
        op: impl IntoOneTimeExecutor<'_>,
    ) -> Result<<Repo as EsRepo>::Entity, <Repo as EsRepo>::Err> {
        let executor = op.into_executor();
        let rows = executor.fetch_all(self.inner).await?;
        Ok(EntityEvents::load_first(rows.into_iter())?)
    }

    async fn fetch_n_inner(
        self,
        op: impl IntoOneTimeExecutor<'_>,
        first: usize,
    ) -> Result<(Vec<<Repo as EsRepo>::Entity>, bool), <Repo as EsRepo>::Err> {
        let executor = op.into_executor();
        let rows = executor.fetch_all(self.inner).await?;
        Ok(EntityEvents::load_n(rows.into_iter(), first)?)
    }
}

impl<'q, Repo, F, A> EsQuery<'q, Repo, EsQueryFlavorFlat, F, A>
where
    Repo: EsRepo,
    <<<Repo as EsRepo>::Entity as EsEntity>::Event as EsEvent>::EntityId: Unpin,
    F: FnMut(
            sqlx::postgres::PgRow,
        ) -> Result<
            GenericEvent<<<<Repo as EsRepo>::Entity as EsEntity>::Event as EsEvent>::EntityId>,
            sqlx::Error,
        > + Send,
    A: 'q + Send + sqlx::IntoArguments<'q, sqlx::Postgres>,
{
    /// Executes the query and returns an optional flat entity.
    pub async fn fetch_optional(
        self,
        op: impl IntoOneTimeExecutor<'_>,
    ) -> Result<Option<<Repo as EsRepo>::Entity>, <Repo as EsRepo>::Err> {
        self.fetch_optional_inner(op).await
    }

    /// Executes the query and returns a single flat entity.
    pub async fn fetch_one(
        self,
        op: impl IntoOneTimeExecutor<'_>,
    ) -> Result<<Repo as EsRepo>::Entity, <Repo as EsRepo>::Err> {
        self.fetch_one_inner(op).await
    }

    /// Executes the query and returns up to `first` flat paginated entities.
    pub async fn fetch_n(
        self,
        op: impl IntoOneTimeExecutor<'_>,
        first: usize,
    ) -> Result<(Vec<<Repo as EsRepo>::Entity>, bool), <Repo as EsRepo>::Err> {
        self.fetch_n_inner(op, first).await
    }
}

impl<'q, Repo, F, A> EsQuery<'q, Repo, EsQueryFlavorNested, F, A>
where
    Repo: EsRepo,
    <<<Repo as EsRepo>::Entity as EsEntity>::Event as EsEvent>::EntityId: Unpin,
    F: FnMut(
            sqlx::postgres::PgRow,
        ) -> Result<
            GenericEvent<<<<Repo as EsRepo>::Entity as EsEntity>::Event as EsEvent>::EntityId>,
            sqlx::Error,
        > + Send,
    A: 'q + Send + sqlx::IntoArguments<'q, sqlx::Postgres>,
{
    /// Executes the query and returns an optional nested entity with all relationships loaded.
    pub async fn fetch_optional<OP>(
        self,
        op: &mut OP,
    ) -> Result<Option<<Repo as EsRepo>::Entity>, <Repo as EsRepo>::Err>
    where
        OP: AtomicOperation,
    {
        let Some(entity) = self.fetch_optional_inner(&mut *op).await? else {
            return Ok(None);
        };
        let mut entities = [entity];
        <Repo as EsRepo>::load_all_nested_in_op(op, &mut entities).await?;
        let [entity] = entities;
        Ok(Some(entity))
    }

    /// Executes the query and returns a single nested entity with all relationships loaded.
    pub async fn fetch_one<OP>(
        self,
        op: &mut OP,
    ) -> Result<<Repo as EsRepo>::Entity, <Repo as EsRepo>::Err>
    where
        OP: AtomicOperation,
    {
        let entity = self.fetch_one_inner(&mut *op).await?;
        let mut entities = [entity];
        <Repo as EsRepo>::load_all_nested_in_op(op, &mut entities).await?;
        let [entity] = entities;
        Ok(entity)
    }

    /// Executes the query and returns up to `first` nested entities with all relationships loaded.
    pub async fn fetch_n<OP>(
        self,
        op: &mut OP,
        first: usize,
    ) -> Result<(Vec<<Repo as EsRepo>::Entity>, bool), <Repo as EsRepo>::Err>
    where
        OP: AtomicOperation,
    {
        let (mut entities, more) = self.fetch_n_inner(&mut *op, first).await?;
        <Repo as EsRepo>::load_all_nested_in_op(op, &mut entities).await?;
        Ok((entities, more))
    }
}
