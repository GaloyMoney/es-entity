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
async fn transient_context_is_not_persisted() -> anyhow::Result<()> {
    let mut ctx = es_entity::EventContext::current();
    ctx.insert("persisted_key", &"persisted-value").unwrap();
    ctx.insert_transient("transient_key", &"transient-value")
        .unwrap();

    let pool = helpers::init_pool().await?;
    let users = Users::new(pool.clone());

    let id = UserId::new();
    let new_user = NewUser::builder()
        .id(id)
        .name("TransientContext")
        .build()
        .unwrap();
    let mut user = users.create(new_user).await?;

    // Also cover the update path (`EntityEvents::push`)
    assert!(user.update_name("TransientContextUpdated").did_execute());
    users.update(&mut user).await?;

    let contexts: Vec<Option<serde_json::Value>> =
        sqlx::query_scalar("SELECT context FROM user_events WHERE id = $1 ORDER BY sequence")
            .bind(uuid::Uuid::from(id))
            .fetch_all(&pool)
            .await?;

    assert_eq!(contexts.len(), 2);

    if cfg!(feature = "event-context") {
        for context in contexts {
            let context = context.expect("context column should be populated");
            assert_eq!(
                context.get("persisted_key"),
                Some(&serde_json::json!("persisted-value"))
            );
            // The transient entry must never reach the database
            assert!(context.get("transient_key").is_none());
        }
    } else {
        // Without the `event-context` feature the repo does not persist contexts
        assert!(contexts.iter().all(Option::is_none));
    }

    Ok(())
}
