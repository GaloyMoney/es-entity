mod helpers;

use es_entity::*;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

es_entity::entity_id! { ProbeId }

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "ProbeId")]
pub enum ProbeEvent {
    Initialized { id: ProbeId },
    Named { name: String },
    NicknameSet { nickname: String },
    Tagged { tag: String },
}

#[derive(EsEntity)]
pub struct Probe {
    pub id: ProbeId,
    pub name: String,
    pub nickname: Option<String>,
    #[es_entity(default)]
    pub tags: Vec<String>,
    events: EntityEvents<ProbeEvent>,
}

impl TryFromEvents<ProbeEvent> for Probe {
    fn try_from_events(events: EntityEvents<ProbeEvent>) -> Result<Self, EntityHydrationError> {
        let mut hydrator = Probe::hydrator();
        let mut tags = Vec::new();
        for event in events.iter_all() {
            match event {
                ProbeEvent::Initialized { id } => {
                    hydrator.id(*id);
                }
                ProbeEvent::Named { name } => {
                    hydrator.name(name.clone());
                }
                ProbeEvent::NicknameSet { nickname } => {
                    hydrator.nickname(nickname.clone());
                }
                ProbeEvent::Tagged { tag } => {
                    tags.push(tag.clone());
                    hydrator.tags(tags.clone());
                }
            }
        }
        hydrator.build(events)
    }
}

pub struct NewProbe;

impl IntoEvents<ProbeEvent> for NewProbe {
    type IdSource = FromRepo;

    fn into_events(
        self,
        builder: EntityEventsBuilder<ProbeEvent, FromRepo>,
    ) -> EntityEvents<ProbeEvent> {
        let id = builder.id();
        builder.initial_events([ProbeEvent::Initialized { id }])
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "Probe",
    tbl = "hydration_probes",
    events_tbl = "hydration_probe_events"
)]
pub struct Probes {
    pool: PgPool,
}

async fn plant(pool: &PgPool, events: Vec<serde_json::Value>) -> anyhow::Result<ProbeId> {
    let id = ProbeId::new();
    sqlx::query("INSERT INTO hydration_probes (id, created_at) VALUES ($1, NOW())")
        .bind(id)
        .execute(pool)
        .await?;
    for (index, mut event) in events.into_iter().enumerate() {
        if event["type"] == "initialized" {
            event["id"] = serde_json::json!(id);
        }
        sqlx::query(
            "INSERT INTO hydration_probe_events (id, sequence, event_type, event, recorded_at) \
             VALUES ($1, $2, $3, $4, NOW())",
        )
        .bind(id)
        .bind(index as i32 + 1)
        .bind(event["type"].as_str().unwrap_or_default().to_string())
        .bind(event)
        .execute(pool)
        .await?;
    }
    Ok(id)
}

#[tokio::test]
async fn find_by_id_reports_the_required_field_no_event_set() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let probes = Probes { pool: pool.clone() };
    let id = plant(&pool, vec![serde_json::json!({ "type": "initialized" })]).await?;

    let err = match probes.find_by_id(id).await {
        Ok(_) => panic!("expected MissingField, got Ok"),
        Err(e) => e,
    };
    let fatal = match &err {
        Fault::Fatal(fatal) => fatal,
        other => panic!("expected Fatal(CorruptState), got: {other:?}"),
    };
    assert_eq!(fatal.kind, FatalKind::CorruptState);
    assert!(
        matches!(
            std::error::Error::source(fatal).and_then(|s| s.downcast_ref::<EntityHydrationError>()),
            Some(EntityHydrationError::MissingField {
                entity: "Probe",
                field: "name"
            })
        ),
        "unexpected error: {err}"
    );
    Ok(())
}

#[tokio::test]
async fn find_by_id_hydrates_unset_option_and_default_fields() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let probes = Probes { pool: pool.clone() };
    let id = plant(
        &pool,
        vec![
            serde_json::json!({ "type": "initialized" }),
            serde_json::json!({ "type": "named", "name": "probe" }),
        ],
    )
    .await?;

    let probe = probes.find_by_id(id).await?;
    assert_eq!(probe.name, "probe");
    assert_eq!(probe.nickname, None);
    assert!(probe.tags.is_empty());
    Ok(())
}

#[tokio::test]
async fn find_by_id_hydrates_set_option_and_default_fields() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let probes = Probes { pool: pool.clone() };
    let id = plant(
        &pool,
        vec![
            serde_json::json!({ "type": "initialized" }),
            serde_json::json!({ "type": "named", "name": "probe" }),
            serde_json::json!({ "type": "nickname_set", "nickname": "p" }),
            serde_json::json!({ "type": "tagged", "tag": "a" }),
            serde_json::json!({ "type": "tagged", "tag": "b" }),
        ],
    )
    .await?;

    let probe = probes.find_by_id(id).await?;
    assert_eq!(probe.nickname.as_deref(), Some("p"));
    assert_eq!(probe.tags, ["a", "b"]);
    Ok(())
}
