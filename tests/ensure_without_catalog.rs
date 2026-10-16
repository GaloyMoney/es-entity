mod helpers;

use es_entity::*;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

es_entity::entity_id! { DoohickeyId }

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "DoohickeyId")]
pub enum DoohickeyEvent {
    Initialized { id: DoohickeyId },
}

#[derive(EsEntity)]
pub struct Doohickey {
    pub id: DoohickeyId,
    events: EntityEvents<DoohickeyEvent>,
}

impl TryFromEvents<DoohickeyEvent> for Doohickey {
    fn try_from_events(events: EntityEvents<DoohickeyEvent>) -> Result<Self, EntityHydrationError> {
        Ok(Doohickey {
            id: *events.id(),
            events,
        })
    }
}

pub struct NewDoohickey {
    pub id: DoohickeyId,
}

impl IntoEvents<DoohickeyEvent> for NewDoohickey {
    type IdSource = FromCaller;

    fn into_events(
        self,
        builder: EntityEventsBuilder<DoohickeyEvent, FromCaller>,
    ) -> EntityEvents<DoohickeyEvent> {
        builder.initial_events(self.id, [DoohickeyEvent::Initialized { id: self.id }])
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "Doohickey",
    id(source = caller),
    migrations_dir = "no_such_migrations_dir"
)]
pub struct Doohickeys {
    pool: PgPool,
}

#[tokio::test]
async fn a_table_without_a_catalog_is_ensured_by_id() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = Doohickeys { pool: pool.clone() };
    let id = DoohickeyId::new();

    let mut op = DbOp::init(&pool).await?;
    let first: Result<Ensured<Doohickey>, RepoFault> = repo
        .create_or_find_in_op(&mut op, NewDoohickey { id })
        .await;
    let second: Result<Ensured<Doohickey>, RepoFault> = repo
        .create_or_find_in_op(&mut op, NewDoohickey { id })
        .await;
    op.commit().await?;

    assert!(first?.is_created());
    let Ensured::Found(found) = second? else {
        panic!("the same id must find")
    };
    assert_eq!(found.id, id);
    Ok(())
}
