mod helpers;

use es_entity::errlanes::Fail;
use es_entity::*;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

es_entity::entity_id! { EnsureSoftBoxId, EnsureSoftNoteId }

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "EnsureSoftNoteId")]
pub enum EnsureSoftNoteEvent {
    Initialized {
        id: EnsureSoftNoteId,
        box_id: EnsureSoftBoxId,
    },
}

#[derive(EsEntity)]
pub struct EnsureSoftNote {
    pub id: EnsureSoftNoteId,
    pub box_id: EnsureSoftBoxId,
    events: EntityEvents<EnsureSoftNoteEvent>,
}

impl TryFromEvents<EnsureSoftNoteEvent> for EnsureSoftNote {
    fn try_from_events(
        events: EntityEvents<EnsureSoftNoteEvent>,
    ) -> Result<Self, EntityHydrationError> {
        let box_id = events
            .iter_all()
            .map(|EnsureSoftNoteEvent::Initialized { box_id, .. }| *box_id)
            .next()
            .expect("an initialized event");
        Ok(EnsureSoftNote {
            id: *events.id(),
            box_id,
            events,
        })
    }
}

pub struct NewEnsureSoftNote {
    pub box_id: EnsureSoftBoxId,
}

impl IntoEvents<EnsureSoftNoteEvent> for NewEnsureSoftNote {
    type IdSource = FromRepo;

    fn into_events(
        self,
        builder: EntityEventsBuilder<EnsureSoftNoteEvent, FromRepo>,
    ) -> EntityEvents<EnsureSoftNoteEvent> {
        let id = builder.id();
        builder.initial_events(vec![EnsureSoftNoteEvent::Initialized {
            id,
            box_id: self.box_id,
        }])
    }
}

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "EnsureSoftBoxId")]
pub enum EnsureSoftBoxEvent {
    Initialized { id: EnsureSoftBoxId, label: String },
}

#[derive(EsEntity)]
pub struct EnsureSoftBox {
    pub id: EnsureSoftBoxId,
    pub label: String,
    events: EntityEvents<EnsureSoftBoxEvent>,
    #[es_entity(nested)]
    notes: Nested<EnsureSoftNote>,
}

impl EnsureSoftBox {
    pub fn add_note(&mut self) {
        self.notes.add_new(NewEnsureSoftNote { box_id: self.id });
    }

    pub fn n_notes(&self) -> usize {
        self.notes.len_persisted()
    }
}

impl TryFromEvents<EnsureSoftBoxEvent> for EnsureSoftBox {
    fn try_from_events(
        events: EntityEvents<EnsureSoftBoxEvent>,
    ) -> Result<Self, EntityHydrationError> {
        let (id, label) = events
            .iter_all()
            .map(|EnsureSoftBoxEvent::Initialized { id, label }| (*id, label.clone()))
            .next()
            .expect("an initialized event");
        Ok(EnsureSoftBox {
            id,
            label,
            events,
            notes: Nested::default(),
        })
    }
}

pub struct NewEnsureSoftBox {
    pub id: EnsureSoftBoxId,
    pub label: String,
}

impl IntoEvents<EnsureSoftBoxEvent> for NewEnsureSoftBox {
    type IdSource = FromCaller;

    fn into_events(
        self,
        builder: EntityEventsBuilder<EnsureSoftBoxEvent, FromCaller>,
    ) -> EntityEvents<EnsureSoftBoxEvent> {
        builder.initial_events(
            self.id,
            [EnsureSoftBoxEvent::Initialized {
                id: self.id,
                label: self.label,
            }],
        )
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "EnsureSoftBox",
    id(source = caller),
    delete = "soft",
    columns(label(ty = "String"))
)]
pub struct EnsureSoftBoxes {
    pool: PgPool,

    #[es_repo(nested)]
    notes: EnsureSoftNotes,
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "EnsureSoftNote",
    delete = "soft",
    columns(box_id(ty = "EnsureSoftBoxId", update(persist = false), parent))
)]
pub struct EnsureSoftNotes {
    pool: PgPool,
}

#[tokio::test]
async fn deleted_holder_with_children_is_rejected() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureSoftBoxes {
        pool: pool.clone(),
        notes: EnsureSoftNotes { pool: pool.clone() },
    };
    let label = format!("label-{}", EnsureSoftBoxId::new());

    let mut op = DbOp::init(&pool).await?;
    let id = EnsureSoftBoxId::new();
    let Ensured::Created(mut created) = repo
        .create_or_find_in_op(
            &mut op,
            NewEnsureSoftBox {
                id,
                label: label.clone(),
            },
        )
        .await?
    else {
        panic!("expected Created");
    };
    created.add_note();
    created.add_note();
    repo.update_in_op(&mut op, &mut created).await?;
    let loaded = repo.find_by_id_in_op(&mut op, id).await?;
    assert_eq!(loaded.n_notes(), 2);
    repo.delete_in_op(&mut op, loaded).await?;

    let err = repo
        .create_or_find_in_op(
            &mut op,
            NewEnsureSoftBox {
                id: EnsureSoftBoxId::new(),
                label,
            },
        )
        .await
        .err()
        .expect("a deleted holder must reject");
    let Fail::Rejected(EnsureSoftBoxEnsureRejection::KeysHeldByDeleted { ids, keys }) = err else {
        panic!("expected KeysHeldByDeleted, got {err:?}");
    };
    assert_eq!(ids, vec![id]);
    assert_eq!(keys, vec!["label"]);
    Ok(())
}

#[tokio::test]
async fn null_deleted_row_is_not_live() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureSoftBoxes {
        pool: pool.clone(),
        notes: EnsureSoftNotes { pool: pool.clone() },
    };
    let label = format!("label-{}", EnsureSoftBoxId::new());

    let mut op = DbOp::init(&pool).await?;
    let id = EnsureSoftBoxId::new();
    repo.create_in_op(
        &mut op,
        NewEnsureSoftBox {
            id,
            label: label.clone(),
        },
    )
    .await?;
    sqlx::query("UPDATE ensure_soft_boxes SET deleted = NULL WHERE id = $1")
        .bind(id)
        .execute(op.as_executor())
        .await?;
    assert!(repo.maybe_find_by_id_in_op(&mut op, id).await?.is_none());

    let err = repo
        .create_or_find_in_op(
            &mut op,
            NewEnsureSoftBox {
                id: EnsureSoftBoxId::new(),
                label,
            },
        )
        .await
        .err()
        .expect("a NULL deleted holder must reject");
    let Fail::Rejected(EnsureSoftBoxEnsureRejection::KeysHeldByDeleted { ids, .. }) = err else {
        panic!("expected KeysHeldByDeleted, got {err:?}");
    };
    assert_eq!(ids, vec![id]);
    Ok(())
}
