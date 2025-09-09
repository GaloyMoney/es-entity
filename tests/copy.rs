mod helpers;

use chrono::Utc;
use es_entity::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "uuid::Uuid", event_context)]
pub enum UserEvent {
    Initialized { id: uuid::Uuid, name: String },
    NameUpdated { name: String },
}

// #[tokio::test]
async fn copy_events() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let id = uuid::Uuid::now_v7();
    let mut events = EntityEvents::init(
        id,
        [
            UserEvent::Initialized {
                id,
                name: "name".to_string(),
            },
            UserEvent::NameUpdated {
                name: "new_name".to_string(),
            },
        ],
    );
    persist_events(&pool, &mut events).await?;
    Ok(())
}

use sqlx::postgres::PgPool;

async fn persist_events(
    pool: &sqlx::PgPool,
    events: &mut es_entity::EntityEvents<UserEvent>,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;

    // Perform the COPY operation in a way that releases the borrow
    let rows_copied = extract_concurrent_modification({
        let mut copy = tx
            .copy_in_raw(
                "COPY user_events (id, sequence, event_type, event, recorded_at) 
                 FROM STDIN WITH (FORMAT text)",
            )
            .await?;
        let id = events.id();
        let offset = events.len_persisted();
        let serialized_events = events.serialize_new_events();

        for (idx, event) in serialized_events.into_iter().enumerate() {
            let event_type = event
                .get("type")
                .and_then(es_entity::prelude::serde_json::Value::as_str)
                .expect("Could not read event type")
                .to_owned();
            let row = format!(
                "{}\t{}\t{}\t{}\t{}\n",
                id,
                offset + 1 + idx,
                event_type,
                serde_json::to_string(&event).expect("event to string"),
                Utc::now(),
            );

            copy.send(row.as_bytes()).await?;
        }

        copy.finish().await
    })?;

    // Now we can safely commit
    tx.commit().await?;

    println!("Copied {} rows", rows_copied);
    Ok(())
}
fn extract_concurrent_modification<T>(
    res: Result<T, sqlx::Error>,
) -> Result<T, es_entity::EsRepoError> {
    match res {
        Ok(entity) => Ok(entity),
        Err(sqlx::Error::Database(db_error)) if db_error.is_unique_violation() => Err(
            es_entity::EsRepoError::from(es_entity::EsEntityError::ConcurrentModification),
        ),
        Err(err) => Err(es_entity::EsRepoError::from(err)),
    }
}
