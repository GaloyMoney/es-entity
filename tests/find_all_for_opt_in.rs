mod helpers;

use es_entity::*;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

es_entity::entity_id! { OptInEntryId }
es_entity::entity_id! { OptInAccountId }

/// A key type that is a valid `list_for` column (`sqlx::Type + Encode`) but
/// is not `Hash + Eq`, so it can never key a map.
#[derive(Debug, Clone, PartialEq, sqlx::Type)]
#[sqlx(transparent)]
pub struct Visibility(String);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntryValues {
    pub account_id: OptInAccountId,
    pub visibility: String,
}

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "OptInEntryId")]
pub enum OptInEntryEvent {
    Initialized {
        id: OptInEntryId,
        values: EntryValues,
    },
}

#[derive(EsEntity)]
pub struct OptInEntry {
    pub id: OptInEntryId,
    values: EntryValues,
    events: EntityEvents<OptInEntryEvent>,
}

impl OptInEntry {
    pub fn values(&self) -> &EntryValues {
        &self.values
    }
}

impl TryFromEvents<OptInEntryEvent> for OptInEntry {
    fn try_from_events(
        events: EntityEvents<OptInEntryEvent>,
    ) -> Result<Self, EntityHydrationError> {
        let mut h = OptInEntry::hydrator();
        for event in events.iter_all() {
            match event {
                OptInEntryEvent::Initialized { id, values } => {
                    h.id(*id).values(values.clone());
                }
            }
        }
        h.build(events)
    }
}

#[derive(Debug, Clone)]
pub struct NewOptInEntry {
    pub account_id: OptInAccountId,
    pub visibility: String,
}

impl NewOptInEntry {
    fn visibility_key(&self) -> Visibility {
        Visibility(self.visibility.clone())
    }
}

impl IntoEvents<OptInEntryEvent> for NewOptInEntry {
    type IdSource = FromRepo;

    fn into_events(
        self,
        builder: EntityEventsBuilder<OptInEntryEvent, FromRepo>,
    ) -> EntityEvents<OptInEntryEvent> {
        let id = builder.id();
        builder.initial_events([OptInEntryEvent::Initialized {
            id,
            values: EntryValues {
                account_id: self.account_id,
                visibility: self.visibility,
            },
        }])
    }
}

pub mod plain {
    use super::*;

    #[derive(EsRepo, Debug)]
    #[es_repo(
        entity = "OptInEntry",
        tbl = "opt_in_entries",
        events_tbl = "opt_in_entry_events",
        columns(
            account_id(
                ty = "OptInAccountId",
                list_for(by(created_at)),
                update(persist = false)
            ),
            visibility(
                ty = "Visibility",
                list_for(by(created_at)),
                create(accessor = "visibility_key()"),
                update(persist = false)
            ),
        )
    )]
    pub struct PlainEntries {
        pub pool: PgPool,
    }
}

#[tokio::test]
async fn list_for_without_the_flag_compiles_for_values_struct_and_non_hash_keys()
-> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = plain::PlainEntries { pool };
    let account_id = OptInAccountId::new();
    let visibility = format!("v-{}", OptInEntryId::new());
    let created = repo
        .create(NewOptInEntry {
            account_id,
            visibility: visibility.clone(),
        })
        .await?;
    let args = || PaginatedQueryArgs {
        first: 10,
        after: None,
    };

    let by_account = repo
        .list_for_account_id_by_created_at(account_id, args(), ListDirection::Descending)
        .await?;
    assert_eq!(by_account.entities().len(), 1);
    assert_eq!(by_account.entities()[0].id, created.id);

    let by_visibility = repo
        .list_for_visibility_by_created_at(
            Visibility(visibility),
            args(),
            ListDirection::Descending,
        )
        .await?;
    assert_eq!(by_visibility.entities().len(), 1);
    assert_eq!(by_visibility.entities()[0].id, created.id);

    Ok(())
}

pub mod batch {
    use super::*;

    #[derive(EsRepo, Debug)]
    #[es_repo(
        entity = "OptInEntry",
        tbl = "opt_in_entries",
        events_tbl = "opt_in_entry_events",
        columns(
            account_id(
                ty = "OptInAccountId",
                list_for(by(created_at), find_all_for),
                update(persist = false, accessor = "values().account_id")
            ),
            visibility(
                ty = "Visibility",
                list_for(by(created_at)),
                create(accessor = "visibility_key()"),
                update(persist = false)
            ),
        )
    )]
    pub struct BatchEntries {
        pub pool: PgPool,
    }
}

#[tokio::test]
async fn flag_with_update_accessor_reads_the_key_from_the_values_struct() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = batch::BatchEntries { pool };
    let (a, b, c) = (
        OptInAccountId::new(),
        OptInAccountId::new(),
        OptInAccountId::new(),
    );
    let new = |account_id| NewOptInEntry {
        account_id,
        visibility: "public".to_string(),
    };
    let a1 = repo.create(new(a)).await?.id;
    let a2 = repo.create(new(a)).await?.id;
    let b1 = repo.create(new(b)).await?.id;

    let found: std::collections::HashMap<OptInAccountId, Vec<OptInEntry>> = repo
        .find_all_for_account_ids(&[a, b, c], ListDirection::Ascending)
        .await?;

    assert_eq!(found.len(), 2);
    let ids = |key: &OptInAccountId| found[key].iter().map(|e| e.id).collect::<Vec<_>>();
    assert_eq!(ids(&a), vec![a1, a2]);
    assert_eq!(ids(&b), vec![b1]);
    assert!(!found.contains_key(&c));
    assert_eq!(found[&a][0].values().account_id, a);

    Ok(())
}
