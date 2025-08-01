use crate::{
    events::{EntityEvents, GenericEvent},
    traits::*,
};

/// Controls sorting order of listing the entities from the database
///
/// ListDirection is used to specify the sorting order of entities when listing them using [crate::EsRepo]
/// methods like `list_by` and `list_for`.
///
/// # Examples
///
/// ```ignore
/// // List users by ID in ascending order (oldest first)
/// let paginated_users = users.list_by_id(
///     PaginatedQueryArgs { first: 5, after: None },
///     ListDirection::Ascending, // or just Default::default()
/// ).await?
///
/// // List users by name in descending order (Z to A)
/// users.list_by_name(
///     PaginatedQueryArgs { first: 10, after: None },
///     ListDirection::Descending,
/// ).await?
/// ```
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

/// A cursor-based pagination structure for efficiently paginating through large datasets
///
/// The `PaginatedQueryArgs<T>` encapsulates a `size` field and an optional `after` field.
/// `<T>` parameter represents cursor type, which depends on the sorting field (e.g., `UsersByIdCursor`, `UsersByNameCursor`).
/// Used in [crate::EsRepo] functions like `list_by`, `list_for`, `find_many`.
///
/// # Examples
///
/// ```ignore
/// // Initial query - fetch first 10 users
/// let query_args = PaginatedQueryArgs {
///     first: 10,
///     after: None, // Start from beginning
/// };
///
/// // Execute query using `query_args` argument of `PaginatedQueryArgs` type
/// let result = users.list_by_id(query_args, ListDirection::Ascending).await?;
///
/// // Continue pagination using the updated `next_query_args` of `PaginatedQueryArgs` type
/// if result.has_next_page {
///     let next_query_args = PaginatedQueryArgs {
///         first: 10,
///         after: result.end_cursor, // Use cursor from previous result
///     };
///     let next_result = users.list_by_id(next_query_args, ListDirection::Ascending).await?;
/// }
/// ```
#[derive(Debug)]
pub struct PaginatedQueryArgs<T: std::fmt::Debug> {
    /// Specifies the number of queries to fetch per query
    pub first: usize,
    /// Specifies the cursor/marker to start from for current query
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

/// Return type for paginated queries containing entities and pagination metadata
///
/// `PaginatedQueryRet` contains the fetched entities and utilities for continuing pagination.
/// Returned by the [crate::EsRepo] functions like `list_by`, `list_for`, `find_many`.
/// Used with [crate::PaginatedQueryArgs] to perform consistent and efficient pagination
///
/// # Examples
///
/// ```ignore
/// let query_args = PaginatedQueryArgs {
///     first: 10,
///     after: None,
/// };
///
/// // Execute query and get the `result` of type `PaginatedQueryRet`
/// let result = users.list_by_id(query_args, ListDirection::Ascending).await?;
///
/// // Continue pagination using the `next_query_args` argument updated with `PaginatedQueryRet`
/// // Will continue only if 'has_next_page` returned from `result` is true
/// if result.has_next_page {
///     let next_query_args = PaginatedQueryArgs {
///         first: 10,
///         after: result.end_cursor, // update with 'end_cursor' of previous `PaginatedQueryRet` result
///     };
///     let next_result = users.list_by_id(next_query_args, ListDirection::Ascending).await?;
/// }
///
/// // Or use PaginatedQueryRet::into_next_query() convenience method
/// if let Some(next_query_args) = result.into_next_query() {
///     let next_result = users.list_by_id(next_query_args, ListDirection::Ascending).await?;
/// }
/// ```
pub struct PaginatedQueryRet<T, C> {
    /// [Vec] for the fetched `entities` by the paginated query
    pub entities: Vec<T>,
    /// [bool] for indicating if the list has been exhausted or more entities can be fetched
    pub has_next_page: bool,
    /// cursor on the last entity fetched to continue paginated queries.
    pub end_cursor: Option<C>,
}

impl<T, C> PaginatedQueryRet<T, C> {
    /// Convenience method to create next query args if more pages are available
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

pub struct EsQuery<'q, Entity, Err, DB, F, A>
where
    DB: sqlx::Database,
{
    inner: sqlx::query::Map<'q, DB, F, A>,
    _entity: std::marker::PhantomData<Entity>,
    _err: std::marker::PhantomData<Err>,
}

impl<'q, Entity, Err, DB, F, A> EsQuery<'q, Entity, Err, DB, F, A>
where
    Entity: EsEntity,
    <<Entity as EsEntity>::Event as EsEvent>::EntityId: Unpin,
    Err: From<sqlx::Error> + From<crate::error::EsEntityError>,
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
            _entity: std::marker::PhantomData,
            _err: std::marker::PhantomData,
        }
    }

    pub async fn fetch_optional(
        self,
        executor: impl sqlx::Executor<'_, Database = DB>,
    ) -> Result<Option<Entity>, Err> {
        let rows = self.inner.fetch_all(executor).await?;
        if rows.is_empty() {
            return Ok(None);
        }

        Ok(Some(EntityEvents::load_first(rows.into_iter())?))
    }

    pub async fn fetch_one(
        self,
        executor: impl sqlx::Executor<'_, Database = DB>,
    ) -> Result<Entity, Err> {
        let rows = self.inner.fetch_all(executor).await?;
        Ok(EntityEvents::load_first(rows.into_iter())?)
    }

    pub async fn fetch_n(
        self,
        executor: impl sqlx::Executor<'_, Database = DB>,
        first: usize,
    ) -> Result<(Vec<Entity>, bool), Err> {
        let rows = self.inner.fetch_all(executor).await?;
        Ok(EntityEvents::load_n(rows.into_iter(), first)?)
    }
}
