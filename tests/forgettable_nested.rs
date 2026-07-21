//! Cascade-delete PII scrub for forgettable **nested children**.
//!
//! Soft-deleting a parent cascades to its direct nested children. When a child
//! is forgettable, that cascade must also scrub the child's forgettable data —
//! delete its payload rows and NULL its forgettable index columns — mirroring
//! the parent's own delete scrub. Otherwise soft-deleting the parent silently
//! retains the child's PII.

mod helpers;

use derive_builder::Builder;
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

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
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
        let mut builder = AccountHolderBuilder::default();
        for event in events.iter_all() {
            match event {
                AccountHolderEvent::Initialized {
                    id,
                    account_id,
                    name,
                    email,
                } => {
                    builder = builder
                        .id(*id)
                        .account_id(*account_id)
                        .name(name.clone())
                        .email(email.clone());
                }
            }
        }
        builder.events(events).build()
    }
}

#[derive(Debug, Builder)]
pub struct NewAccountHolder {
    pub id: AccountHolderId,
    pub account_id: AccountId,
    #[builder(setter(into))]
    pub name: String,
    #[builder(setter(into))]
    pub email: String,
}

impl NewAccountHolder {
    pub fn builder() -> NewAccountHolderBuilder {
        NewAccountHolderBuilder::default()
    }
}

impl IntoEvents<AccountHolderEvent> for NewAccountHolder {
    fn into_events(self) -> EntityEvents<AccountHolderEvent> {
        EntityEvents::init(
            self.id,
            [AccountHolderEvent::Initialized {
                id: self.id,
                account_id: self.account_id,
                name: Forgettable::new(self.name),
                email: Forgettable::new(self.email),
            }],
        )
    }
}

// The parent.
#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "AccountId")]
pub enum AccountEvent {
    Initialized { id: AccountId },
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
pub struct Account {
    pub id: AccountId,
    events: EntityEvents<AccountEvent>,

    #[es_entity(nested)]
    #[builder(default)]
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
        let mut builder = AccountBuilder::default();
        for event in events.iter_all() {
            match event {
                AccountEvent::Initialized { id } => builder = builder.id(*id),
            }
        }
        builder.events(events).build()
    }
}

#[derive(Debug, Builder)]
pub struct NewAccount {
    pub id: AccountId,
}

impl NewAccount {
    pub fn builder() -> NewAccountBuilder {
        NewAccountBuilder::default()
    }
}

impl IntoEvents<AccountEvent> for NewAccount {
    fn into_events(self) -> EntityEvents<AccountEvent> {
        EntityEvents::init(self.id, [AccountEvent::Initialized { id: self.id }])
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
    let account_id = AccountId::new();
    let holder_id = AccountHolderId::new();
    let email = format!("holder-{holder_id}@example.com");

    let mut account = accounts
        .create(NewAccount::builder().id(account_id).build().unwrap())
        .await?;
    account.add_holder(
        NewAccountHolder::builder()
            .id(holder_id)
            .account_id(account_id)
            .name("Grace Hopper")
            .email(email.clone())
            .build()
            .unwrap(),
    );
    accounts.update(&mut account).await?;

    // Sanity: while live, the child is loaded, its index column holds the
    // value, and a payload row exists.
    let loaded = accounts.find_by_id(account_id).await?;
    assert_eq!(loaded.n_holders(), 1);

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
