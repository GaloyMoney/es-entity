mod entities;
mod helpers;

use std::collections::HashSet;

use entities::contact::*;
use es_entity::*;
use sqlx::PgPool;

/// Same value virtual as `tests/virtual_filter_value.rs`, this time on a
/// partner-scoped repo — its bound `$k` must land *after* the scope
/// column's `$1` and the conjunct must still be scoped correctly, exactly
/// like the bool-polarity virtual's unparameterized conjunct already is in
/// `tests/virtual_filter_column_scoped.rs`.
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "Contact",
    columns(
        partner_id(ty = "PartnerId", scope),
        email(ty = "String"),
        status(ty = "String", list_for(by(created_at))),
        min_flags(
            ty = "i64",
            virtual = "(SELECT COUNT(*) FROM contact_flags f WHERE f.contact_id = contacts.id) >= {value}",
            list_for
        ),
    )
)]
pub struct ScopedVfvContacts {
    pool: PgPool,
}

impl ScopedVfvContacts {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

async fn create_contact(
    repo: &ScopedVfvContacts,
    partner_id: PartnerId,
    status: &str,
) -> anyhow::Result<Contact> {
    let id = ContactId::new();
    Ok(repo
        .create(
            NewContact::builder()
                .id(id)
                .partner_id(partner_id)
                .email(format!("{id}@test.com"))
                .status(status)
                .build()
                .unwrap(),
        )
        .await?)
}

async fn flag(pool: &PgPool, contact_id: ContactId) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO contact_flags (contact_id, flag) VALUES ($1, 'due')")
        .bind(contact_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// A `min_flags: Some(1)` filter under a specific partner scope returns
/// only that partner's qualifying contacts — never a foreign partner's,
/// even when the foreign partner also has a qualifying row with a matching
/// status. The audited `All` escape hatch still sees across every scope.
#[tokio::test]
async fn scoped_value_virtual_stays_within_scope() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let contacts = ScopedVfvContacts::new(pool.clone());

    let partner_a = PartnerId::new();
    let partner_b = PartnerId::new();
    let unique_status = format!("vfv_scoped_{}", ContactId::new());

    let a_flagged = create_contact(&contacts, partner_a, &unique_status).await?;
    flag(&pool, a_flagged.id).await?;
    let a_plain = create_contact(&contacts, partner_a, &unique_status).await?;
    let b_flagged = create_contact(&contacts, partner_b, &unique_status).await?;
    flag(&pool, b_flagged.id).await?;

    let result = contacts
        .list_for_filters(
            partner_a,
            ContactFilters {
                status: Some(unique_status.clone()),
                min_flags: Some(1),
            },
            Sort {
                by: ContactSortBy::Id,
                direction: ListDirection::Ascending,
            },
            PaginatedQueryArgs {
                first: 100,
                after: None,
            },
        )
        .await?;

    let ids: HashSet<_> = result.entities().iter().map(|e| e.id).collect();
    assert_eq!(ids, HashSet::from([a_flagged.id]));
    assert!(!ids.contains(&a_plain.id));
    assert!(!ids.contains(&b_flagged.id));

    let all_result = contacts
        .list_for_filters(
            ContactScope::All,
            ContactFilters {
                status: Some(unique_status),
                min_flags: Some(1),
            },
            Sort {
                by: ContactSortBy::Id,
                direction: ListDirection::Ascending,
            },
            PaginatedQueryArgs {
                first: 100,
                after: None,
            },
        )
        .await?;
    let all_ids: HashSet<_> = all_result.entities().iter().map(|e| e.id).collect();
    assert_eq!(all_ids, HashSet::from([a_flagged.id, b_flagged.id]));

    Ok(())
}
