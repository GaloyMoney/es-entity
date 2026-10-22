//! Cascade-delete PII scrub for forgettable **nested children**.
//!
//! Soft-deleting a parent cascades to its direct nested children. When a child
//! is forgettable, that cascade must also scrub the child's forgettable data —
//! delete its payload rows and NULL its forgettable index columns — mirroring
//! the parent's own delete scrub. Otherwise soft-deleting the parent silently
//! retains the child's PII.

mod helpers;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use es_entity::*;

es_entity::entity_id! { AccountId, AccountHolderId }

// The forgettable nested child. `name` is a forgettable event field (stored in
// the payloads table); `email` is a forgettable index column (materialised,
// nullable).
#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "AccountHolderId")]
pub enum AccountHolderEvent {
    Initialized {
        id: AccountHolderId,
        account_id: AccountId,
        name: Forgettable<String>,
        email: Forgettable<String>,
    },
}

#[derive(EsEntity)]
pub struct AccountHolder {
    pub id: AccountHolderId,
    pub account_id: AccountId,
    pub name: Forgettable<String>,
    pub email: Forgettable<String>,
    events: EntityEvents<AccountHolderEvent>,
}

impl TryFromEvents<AccountHolderEvent> for AccountHolder {
    fn try_from_events(
        events: EntityEvents<AccountHolderEvent>,
    ) -> Result<Self, EntityHydrationError> {
        let mut hydrator = AccountHolder::hydrator();
        for event in events.iter_all() {
            match event {
                AccountHolderEvent::Initialized {
                    id,
                    account_id,
                    name,
                    email,
                } => {
                    hydrator
                        .id(*id)
                        .account_id(*account_id)
                        .name(name.clone())
                        .email(email.clone());
                }
            }
        }
        hydrator.build(events)
    }
}

#[derive(Debug, bon::Builder)]
pub struct NewAccountHolder {
    pub account_id: AccountId,
    #[builder(into)]
    pub name: String,
    #[builder(into)]
    pub email: String,
}

impl IntoEvents<AccountHolderEvent> for NewAccountHolder {
    type IdSource = FromRepo;

    fn into_events(
        self,
        builder: EntityEventsBuilder<AccountHolderEvent, FromRepo>,
    ) -> EntityEvents<AccountHolderEvent> {
        let id = builder.id();
        builder.initial_events([AccountHolderEvent::Initialized {
            id,
            account_id: self.account_id,
            name: Forgettable::new(self.name),
            email: Forgettable::new(self.email),
        }])
    }
}

// The parent.
#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "AccountId")]
pub enum AccountEvent {
    Initialized { id: AccountId },
}

#[derive(EsEntity)]
pub struct Account {
    pub id: AccountId,
    events: EntityEvents<AccountEvent>,

    #[es_entity(nested)]
    holders: Nested<AccountHolder>,
}

impl Account {
    pub fn add_holder(&mut self, holder: NewAccountHolder) {
        self.holders.add_new(holder);
    }

    pub fn n_holders(&self) -> usize {
        self.holders.len_persisted()
    }
}

impl TryFromEvents<AccountEvent> for Account {
    fn try_from_events(events: EntityEvents<AccountEvent>) -> Result<Self, EntityHydrationError> {
        let mut hydrator = Account::hydrator();
        for event in events.iter_all() {
            match event {
                AccountEvent::Initialized { id } => {
                    hydrator.id(*id);
                }
            }
        }
        hydrator.build(events)
    }
}

#[derive(Debug, bon::Builder)]
pub struct NewAccount {}

impl IntoEvents<AccountEvent> for NewAccount {
    type IdSource = FromRepo;

    fn into_events(
        self,
        builder: EntityEventsBuilder<AccountEvent, FromRepo>,
    ) -> EntityEvents<AccountEvent> {
        let id = builder.id();
        builder.initial_events([AccountEvent::Initialized { id }])
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(entity = "Account", delete = "soft")]
pub struct Accounts {
    pool: PgPool,

    #[es_repo(nested)]
    holders: AccountHolders,
}

impl Accounts {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool: pool.clone(),
            holders: AccountHolders::new(pool),
        }
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "AccountHolder",
    forgettable,
    delete = "soft",
    columns(
        account_id(ty = "AccountId", update(persist = false), parent),
        email(ty = "Forgettable<String>", list_by)
    )
)]
pub struct AccountHolders {
    pool: PgPool,
}

impl AccountHolders {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[tokio::test]
async fn cascade_delete_scrubs_forgettable_nested_children() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let accounts = Accounts::new(pool.clone());

    // Create a parent with a forgettable child (both a forgettable event field
    // and a forgettable index column).
    let email = format!("holder-{}@example.com", AccountHolderId::new());

    let mut account = accounts.create(NewAccount::builder().build()).await?;
    let account_id = account.id;
    account.add_holder(
        NewAccountHolder::builder()
            .account_id(account_id)
            .name("Grace Hopper")
            .email(email.clone())
            .build(),
    );
    accounts.update(&mut account).await?;

    // Sanity: while live, the child is loaded, its index column holds the
    // value, and a payload row exists.
    let loaded = accounts.find_by_id(account_id).await?;
    assert_eq!(loaded.n_holders(), 1);
    let holder_id = loaded.holders.iter_persisted().next().unwrap().id;

    let row = sqlx::query!(
        "SELECT email FROM account_holders WHERE id = $1",
        holder_id as AccountHolderId,
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(row.email.as_deref(), Some(email.as_str()));

    let payloads = sqlx::query!(
        "SELECT COUNT(*) AS count FROM account_holders_forgettable_payloads WHERE entity_id = $1",
        holder_id as AccountHolderId,
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(payloads.count, Some(1));

    // Soft-delete the parent — this cascades to the child.
    let account = accounts.find_by_id(account_id).await?;
    accounts.delete(account).await?;

    // The child is soft-deleted...
    let row = sqlx::query!(
        "SELECT deleted, email FROM account_holders WHERE id = $1",
        holder_id as AccountHolderId,
    )
    .fetch_one(&pool)
    .await?;
    assert!(row.deleted);
    // ...its forgettable index column is NULLed...
    assert_eq!(row.email, None);

    // ...and its payload rows are gone.
    let payloads = sqlx::query!(
        "SELECT COUNT(*) AS count FROM account_holders_forgettable_payloads WHERE entity_id = $1",
        holder_id as AccountHolderId,
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(payloads.count, Some(0));

    Ok(())
}
