mod entities;
mod helpers;

use entities::user::*;
use es_entity::*;
use sqlx::PgPool;

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
async fn list_for_filter() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;

    let users = Users::new(pool);

    let PaginatedQueryRet {
        entities,
        has_next_page: _,
        end_cursor: _,
    } = users
        .list_for_filter(
            UsersFilter::NoFilter,
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

    // Create a user with name Alice for testing the filter
    let alice_id = UserId::new();
    let new_alice = NewUser::builder()
        .id(alice_id)
        .name("Alice")
        .build()
        .unwrap();

    users.create(new_alice).await?;

    let filtered_result = users
        .list_for_filter(
            UsersFilter::WithName("Alice".to_string()),
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

    assert!(!filtered_result.entities.is_empty(),);
    for user in &filtered_result.entities {
        assert_eq!(user.name, "Alice",);
    }

    Ok(())
}
