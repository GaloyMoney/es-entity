//! Handle query generation with helper methods for execution

use crate::{
    events::{EntityEvents, GenericEvent},
    one_time_executor::IntoOneTimeExecutor,
    operation::AtomicOperation,
    traits::*,
};

pub struct EsQuery<'q, Repo, Flavor, F, A> {
    inner: sqlx::query::Map<'q, sqlx::Postgres, F, A>,
    _repo: std::marker::PhantomData<Repo>,
    _flavor: std::marker::PhantomData<Flavor>,
}
pub struct EsQueryFlavorFlat;
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
    pub async fn fetch_optional(
        self,
        op: impl IntoOneTimeExecutor<'_>,
    ) -> Result<Option<<Repo as EsRepo>::Entity>, <Repo as EsRepo>::Err> {
        self.fetch_optional_inner(op).await
    }

    pub async fn fetch_one(
        self,
        op: impl IntoOneTimeExecutor<'_>,
    ) -> Result<<Repo as EsRepo>::Entity, <Repo as EsRepo>::Err> {
        self.fetch_one_inner(op).await
    }

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
