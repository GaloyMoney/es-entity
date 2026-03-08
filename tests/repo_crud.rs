mod entities;
mod helpers;

use entities::user::*;
use es_entity::*;
use sqlx::PgPool;

/// Regression test: repo structs with a generic parameter named 'E' must compile
/// without conflicting with the macro's internal error generic.
/// See: https://github.com/galoymoney/es-entity/issues/fix-generic-E-conflict
mod generic_e_repo {
    #![allow(dead_code)]

    use es_entity::*;
    use sqlx::PgPool;

    use crate::entities::user::*;

    pub trait EventMarker: std::fmt::Debug + Send + Sync + 'static {}

    #[derive(EsRepo, Debug)]
    #[es_repo(entity = "User", columns(name(ty = "String", list_for)))]
    pub struct UsersWithGenericE<E: EventMarker> {
        pool: PgPool,
        _marker: std::marker::PhantomData<E>,
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(entity = "User", columns(name(ty = "String", list_for)))]
pub struct Users {
    pool: PgPool,
}

impl Users {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[tokio::test]
async fn create() -> anyhow::Result<()> {
    let mut ctx = es_entity::EventContext::current();
    ctx.insert("test", &"create").unwrap();
    let pool = helpers::init_pool().await?;

    let users = Users::new(pool);

    let new_user = NewUser::builder()
        .id(UserId::new())
        .name("Frank")
        .build()
        .unwrap();

    let mut user = users.create(new_user).await?;

    if user.update_name("Dweezil").did_execute() {
        ctx.insert("test", &"update").unwrap();
        users.update(&mut user).await?;
    }

    let loaded_user = users.find_by_id(user.id).await?;

    assert_eq!(user.name, loaded_user.name);

    Ok(())
}

#[tokio::test]
async fn list_by() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;

    let users = Users::new(pool);

    let new_user = NewUser::builder()
        .id(UserId::new())
        .name("Frank")
        .build()
        .unwrap();

    users.create(new_user).await?;
    let PaginatedQueryRet {
        entities,
        has_next_page: _,
        end_cursor: _,
    } = users
        .list_by_id(
            PaginatedQueryArgs {
                first: 5,
                after: Some(user_cursor::UsersByIdCursor {
                    id: uuid::Uuid::nil().into(),
                }),
            },
            ListDirection::Ascending,
        )
        .await?;
    assert!(!entities.is_empty());
    Ok(())
}

#[tokio::test]
async fn list_for_filters() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;

    let users = Users::new(pool);

    // Test with default filters (no filter) - should return all entities
    let PaginatedQueryRet {
        entities,
        has_next_page: _,
        end_cursor: _,
    } = users
        .list_for_filters(
            UsersFilters::default(),
            Sort {
                by: UsersSortBy::Id,
                direction: ListDirection::Ascending,
            },
            PaginatedQueryArgs {
                first: 10,
                after: None,
            },
        )
        .await?;

    assert!(!entities.is_empty());

    // Create a user with a unique name for testing the filter
    let unique_name = format!("FiltersTest_{}", UserId::new());
    let new_user = NewUser::builder()
        .id(UserId::new())
        .name(&unique_name)
        .build()
        .unwrap();

    users.create(new_user).await?;

    // Test with specific name filter
    let filtered_result = users
        .list_for_filters(
            UsersFilters {
                name: Some(unique_name.clone()),
            },
            Sort {
                by: UsersSortBy::Id,
                direction: ListDirection::Ascending,
            },
            PaginatedQueryArgs {
                first: 10,
                after: None,
            },
        )
        .await?;

    assert_eq!(filtered_result.entities.len(), 1);
    assert_eq!(filtered_result.entities[0].name, unique_name);

    // Test pagination with filters
    let paginated_result = users
        .list_for_filters(
            UsersFilters::default(),
            Sort {
                by: UsersSortBy::Id,
                direction: ListDirection::Ascending,
            },
            PaginatedQueryArgs {
                first: 1,
                after: None,
            },
        )
        .await?;

    assert_eq!(paginated_result.entities.len(), 1);
    assert!(paginated_result.has_next_page);

    // Use cursor for next page
    let next_page = users
        .list_for_filters(
            UsersFilters::default(),
            Sort {
                by: UsersSortBy::Id,
                direction: ListDirection::Ascending,
            },
            PaginatedQueryArgs {
                first: 1,
                after: paginated_result.end_cursor,
            },
        )
        .await?;

    assert_eq!(next_page.entities.len(), 1);
    assert_ne!(paginated_result.entities[0].id, next_page.entities[0].id);

    Ok(())
}

#[tokio::test]
async fn update_all() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let users = Users::new(pool);

    let new_users = vec![
        NewUser::builder()
            .id(UserId::new())
            .name("Alice")
            .build()
            .unwrap(),
        NewUser::builder()
            .id(UserId::new())
            .name("Bob")
            .build()
            .unwrap(),
        NewUser::builder()
            .id(UserId::new())
            .name("Charlie")
            .build()
            .unwrap(),
    ];

    let mut created = users.create_all(new_users).await?;
    assert_eq!(created.len(), 3);

    let _ = created[0].update_name("Alice_updated");
    let _ = created[1].update_name("Bob_updated");
    // Leave created[2] unchanged to test skipping

    let n_events = users.update_all(&mut created).await?;
    assert_eq!(n_events, 2);

    let loaded_alice = users.find_by_id(created[0].id).await?;
    assert_eq!(loaded_alice.name, "Alice_updated");

    let loaded_bob = users.find_by_id(created[1].id).await?;
    assert_eq!(loaded_bob.name, "Bob_updated");

    let loaded_charlie = users.find_by_id(created[2].id).await?;
    assert_eq!(loaded_charlie.name, "Charlie");

    Ok(())
}
