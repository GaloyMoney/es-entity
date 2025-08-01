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

pub struct EsQuery<'q, Entity, Err, F, A> {
    inner: sqlx::query::Map<'q, sqlx::Postgres, F, A>,
    _entity: std::marker::PhantomData<Entity>,
    _err: std::marker::PhantomData<Err>,
}

impl<'q, Entity, Err, F, A> EsQuery<'q, Entity, Err, F, A>
where
    Entity: EsEntity,
    <<Entity as EsEntity>::Event as EsEvent>::EntityId: Unpin,
    Err: From<sqlx::Error> + From<crate::error::EsEntityError>,
    F: FnMut(
            sqlx::postgres::PgRow,
        ) -> Result<
            GenericEvent<<<Entity as EsEntity>::Event as EsEvent>::EntityId>,
            sqlx::Error,
        > + Send,
    A: 'q + Send + sqlx::IntoArguments<'q, sqlx::Postgres>,
{
    pub fn new(query: sqlx::query::Map<'q, sqlx::Postgres, F, A>) -> Self {
        Self {
            inner: query,
            _entity: std::marker::PhantomData,
            _err: std::marker::PhantomData,
        }
    }

    pub async fn fetch_optional<OP>(
        self,
        op: &mut OP,
    ) -> Result<Option<Entity>, Err> 
    where
        OP: for<'o> AtomicOperation<'o>,
    {
        let executor = op.as_executor();
        let rows = self.inner.fetch_all(executor).await?;
        if rows.is_empty() {
            return Ok(None);
        }

        Ok(Some(EntityEvents::load_first(rows.into_iter())?))
    }

    pub async fn fetch_one<OP>(
        self,
        op: &mut OP,
    ) -> Result<Entity, Err>
    where
        OP: for<'o> AtomicOperation<'o>,
    {
        let executor = op.as_executor();
        let rows = self.inner.fetch_all(executor).await?;
        Ok(EntityEvents::load_first(rows.into_iter())?)
    }

    pub async fn fetch_n<OP>(
        self,
        op: &mut OP,
        first: usize,
    ) -> Result<(Vec<Entity>, bool), Err>
    where
        OP: for<'o> AtomicOperation<'o>,
    {
        let executor = op.as_executor();
        let rows = self.inner.fetch_all(executor).await?;
        Ok(EntityEvents::load_n(rows.into_iter(), first)?)
    }
}
