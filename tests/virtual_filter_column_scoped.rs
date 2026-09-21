mod entities;
mod helpers;

use std::collections::HashSet;

use entities::contact::*;
use es_entity::*;
use sqlx::PgPool;

/// Same virtual filter column as `tests/virtual_filter_column.rs`, this time
/// on a partner-scoped repo — a virtual filter conjunct must survive the
/// scope arm exactly like an ordinary physical filter does (the predicate
/// rides in `trailing`, alongside every scope-column arm's conjunct).
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "Contact",
    columns(
        partner_id(ty = "PartnerId", scope),
        email(ty = "String"),
        status(ty = "String", list_for(by(created_at))),
        flagged(
            ty = "bool",
            virtual = "EXISTS (SELECT 1 FROM contact_flags f WHERE f.contact_id = contacts.id)",
            list_for
        ),
    )
)]
pub struct ScopedVfContacts {
    pool: PgPool,
}

impl ScopedVfContacts {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

async fn create_contact(
    repo: &ScopedVfContacts,
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

/// A `flagged: Some(true)` filter under a specific partner scope returns
/// only that partner's flagged contacts — never a foreign partner's, even
/// when the foreign partner also has a flagged row with a matching status.
#[tokio::test]
async fn scoped_virtual_filter_stays_within_scope() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let contacts = ScopedVfContacts::new(pool.clone());

    let partner_a = PartnerId::new();
    let partner_b = PartnerId::new();
    let unique_status = format!("vf_scoped_{}", ContactId::new());

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
                flagged: Some(true),
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

    // The audited `All` escape hatch sees across every scope, same as an
    // ordinary filter would.
    let all_result = contacts
        .list_for_filters(
            ContactScope::All,
            ContactFilters {
                status: Some(unique_status),
                flagged: Some(true),
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
