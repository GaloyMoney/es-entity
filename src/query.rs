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

pub struct EsQuery<'q, Entity, DB, F, A>
where
    DB: sqlx::Database,
{
    inner: sqlx::query::Map<'q, DB, F, A>,
    _phantom: std::marker::PhantomData<Entity>,
}

impl<'q, Entity, DB, F, A> EsQuery<'q, Entity, DB, F, A>
where
    Entity: EsEntity,
    <<Entity as EsEntity>::Event as EsEvent>::EntityId: Unpin,
    DB: sqlx::Database,
    F: FnMut(
            <DB as sqlx::Database>::Row,
        ) -> Result<
            GenericEvent<<<Entity as EsEntity>::Event as EsEvent>::EntityId>,
            sqlx::Error,
        > + Send,
    A: 'q + Send + sqlx::IntoArguments<'q, DB>,
{
    pub fn new(query: sqlx::query::Map<'q, DB, F, A>) -> Self {
        Self {
            inner: query,
            _phantom: std::marker::PhantomData,
        }
    }

    pub async fn fetch_optional<Err>(
        self,
        executor: impl sqlx::Executor<'_, Database = DB>,
    ) -> Result<Option<Entity>, Err>
    where
        Err: From<sqlx::Error> + From<crate::error::EsEntityError>,
    {
        let rows = self.inner.fetch_all(executor).await?;
        if rows.is_empty() {
            return Ok(None);
        }

        Ok(Some(EntityEvents::load_first(rows.into_iter())?))
    }

    pub async fn fetch_one<Err>(
        self,
        executor: impl sqlx::Executor<'_, Database = DB>,
    ) -> Result<Entity, Err>
    where
        Err: From<sqlx::Error> + From<crate::error::EsEntityError>,
    {
        let rows = self.inner.fetch_all(executor).await?;
        Ok(EntityEvents::load_first(rows.into_iter())?)
    }

    pub async fn fetch_n<Err>(
        self,
        executor: impl sqlx::Executor<'_, Database = DB>,
        first: usize,
    ) -> Result<(Vec<Entity>, bool), Err>
    where
        Err: From<sqlx::Error> + From<crate::error::EsEntityError>,
    {
        let rows = self.inner.fetch_all(executor).await?;
        Ok(EntityEvents::load_n(rows.into_iter(), first)?)
    }
}
