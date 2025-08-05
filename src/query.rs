use crate::{
    events::{EntityEvents, GenericEvent},
    traits::*,
};

#[derive(Default, std::fmt::Debug, Clone, Copy)]
pub enum ListDirection {
    #[default]
    Ascending,
    Descending,
}

#[derive(std::fmt::Debug, Clone, Copy)]
pub struct Sort<T> {
    pub by: T,
    pub direction: ListDirection,
}

#[derive(Debug)]
pub struct PaginatedQueryArgs<T: std::fmt::Debug> {
    pub first: usize,
    pub after: Option<T>,
}

impl<T: std::fmt::Debug> Clone for PaginatedQueryArgs<T>
where
    T: Clone,
{
    fn clone(&self) -> Self {
        Self {
            first: self.first,
            after: self.after.clone(),
        }
    }
}

impl<T: std::fmt::Debug> Default for PaginatedQueryArgs<T> {
    fn default() -> Self {
        Self {
            first: 100,
            after: None,
        }
    }
}

pub struct PaginatedQueryRet<T, C> {
    pub entities: Vec<T>,
    pub has_next_page: bool,
    pub end_cursor: Option<C>,
}

impl<T, C> PaginatedQueryRet<T, C> {
    pub fn into_next_query(self) -> Option<PaginatedQueryArgs<C>>
    where
        C: std::fmt::Debug,
    {
        if self.has_next_page {
            Some(PaginatedQueryArgs {
                first: self.entities.len(),
                after: self.end_cursor,
            })
        } else {
            None
        }
    }
}

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
