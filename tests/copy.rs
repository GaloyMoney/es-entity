mod helpers;

use es_entity::*;
use serde::{Deserialize, Serialize};

// async fn persist_events<OP>(
//     &self,
//     op: &mut OP,
//     events: &mut es_entity::EntityEvents<EntityEvent>
// ) -> Result<usize, es_entity::EsRepoError>
// where
//     OP: es_entity::AtomicOperation
// {
//     let id = events.id();
//     let offset = events.len_persisted();
//     let serialized_events = events.serialize_new_events();
//     let events_types = serialized_events.iter().map(|e| e.get("type").and_then(es_entity::prelude::serde_json::Value::as_str).expect("Could not read event type").to_owned()).collect::<Vec<_>>();
//     let now = op.now();

//     let rows =
//         sqlx::query!(
//             "INSERT INTO entity_events (id, recorded_at, sequence, event_type, event) SELECT $1, COALESCE($2, NOW()), ROW_NUMBER() OVER () + $3, unnested.event_type, unnested.event FROM UNNEST($4::TEXT[], $5::JSONB[]) AS unnested(event_type, event) RETURNING recorded_at",
//             id as &EntityId,
//             now,
//             offset as i32,
//             &events_types,
//             &serialized_events,
//         ).fetch_all(op.as_executor()).await?;

//     let recorded_at = rows[0].recorded_at;
//     let n_events = events.mark_new_events_persisted_at(recorded_at);

//     Ok(n_events)
// }

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "uuid::Uuid", event_context)]
pub enum UserEvent {
    Initialized { id: uuid::Uuid, name: String },
    NameUpdated { name: String },
}

#[tokio::test]
async fn copy_events() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut tx = pool.begin().await?;
    let id = uuid::Uuid::now_v7();
    let mut events = EntityEvents::init(
        id,
        [UserEvent::Initialized {
            id,
            name: "name".to_string(),
        }],
    );
    persist_events(&mut tx, &mut events).await?;
    tx.commit().await?;
    Ok(())
}

async fn persist_events(
    op: &mut impl es_entity::AtomicOperation,
    events: &mut es_entity::EntityEvents<UserEvent>,
) -> anyhow::Result<usize> {
    // @ claude - I want to attempt to reformulate the fn above using copy.
    // start by creating the smallest possible possible POC that demonstrates that
    Ok(0)
}
