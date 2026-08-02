mod entities;
mod helpers;

use sqlx::PgPool;

use entities::contact::*;
use es_entity::*;

/// The scoped `Contacts` repo with `sargable_filters` opted in: the
/// scope × filter composition must hold through the specialized per-state
/// query matrix, not just the catch-all fallback (exercised by
/// `scoped_repo.rs`, which keeps the default). The scope column doubles as a
/// `list_for` filter column; under `Only` its filter predicate is skipped —
/// a mismatch short-circuits in Rust, a match is already pinned by the scope
/// predicate.
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "Contact",
    columns(
        partner_id(ty = "PartnerId", scope, list_for(by(created_at))),
        email(ty = "String"),
        status(ty = "String", list_for(by(created_at))),
    ),
    sargable_filters
)]
pub struct Contacts {
    pool: PgPool,
}

impl Contacts {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

async fn seed_contacts(
    repo: &Contacts,
    partner_id: PartnerId,
    specs: &[(&str, &str)],
) -> anyhow::Result<Vec<ContactId>> {
    let mut ids = Vec::new();
    for (email, status) in specs {
        let id = ContactId::new();
        let new = NewContact::builder()
            .id(id)
            .partner_id(partner_id)
            .email(format!("{email}-{id}@test.com"))
            .status(*status)
            .build()
            .unwrap();
        repo.create(new).await?;
        ids.push(id);
    }
    Ok(ids)
}

#[tokio::test]
async fn sargable_scope_column_filter_combinations() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let contacts = Contacts::new(pool);

    let partner_a = PartnerId::new();
    let partner_b = PartnerId::new();
    let ids_a = seed_contacts(
        &contacts,
        partner_a,
        &[("s1", "active"), ("s2", "inactive")],
    )
    .await?;
    seed_contacts(&contacts, partner_b, &[("s3", "active")]).await?;

    let query = || PaginatedQueryArgs {
        first: 100,
        after: None,
    };
    let sort = || Sort {
        by: ContactSortBy::CreatedAt,
        direction: ListDirection::Descending,
    };

    // All + None: unfiltered (shared table — containment, not count)
    let ret = contacts
        .list_for_filters(
            ContactScope::All,
            ContactFilters::default(),
            sort(),
            query(),
        )
        .await?;
    assert!(
        ids_a
            .iter()
            .all(|id| ret.entities.iter().any(|c| c.id == *id))
    );

    // All + Some(a)
    let ret = contacts
        .list_for_filters(
            ContactScope::All,
            ContactFilters {
                partner_id: Some(partner_a),
                ..Default::default()
            },
            sort(),
            query(),
        )
        .await?;
    assert_eq!(ret.entities.len(), 2);
    assert!(ret.entities.iter().all(|c| c.partner_id == partner_a));

    // Only(a) + Some(a): match — collapses into the scope predicate
    let ret = contacts
        .list_for_filters(
            partner_a,
            ContactFilters {
                partner_id: Some(partner_a),
                ..Default::default()
            },
            sort(),
            query(),
        )
        .await?;
    assert_eq!(ret.entities.len(), 2);

    // Only(a) + Some(b): mismatch — empty, short-circuited
    let ret = contacts
        .list_for_filters(
            partner_a,
            ContactFilters {
                partner_id: Some(partner_b),
                ..Default::default()
            },
            sort(),
            query(),
        )
        .await?;
    assert!(ret.entities.is_empty());
    assert!(!ret.has_next_page);

    // multi-filter through the specialized scoped arms (scope-column filter
    // predicate skipped, status predicate present)
    let ret = contacts
        .list_for_filters(
            partner_a,
            ContactFilters {
                partner_id: Some(partner_a),
                status: Some("active".to_string()),
            },
            sort(),
            query(),
        )
        .await?;
    assert_eq!(ret.entities.len(), 1);
    assert_eq!(ret.entities[0].status, "active");
    let ret = contacts
        .list_for_filters(
            partner_a,
            ContactFilters {
                partner_id: Some(partner_b),
                status: Some("active".to_string()),
            },
            sort(),
            query(),
        )
        .await?;
    assert!(ret.entities.is_empty());

    Ok(())
}

#[tokio::test]
async fn sargable_scope_column_filter_cursor_pages() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let contacts = Contacts::new(pool);

    let partner_a = PartnerId::new();
    let partner_b = PartnerId::new();
    let specs: Vec<(&str, &str)> = (0..5).map(|_| ("sp", "active")).collect();
    let ids_a = seed_contacts(&contacts, partner_a, &specs).await?;
    seed_contacts(&contacts, partner_b, &specs).await?;

    // All + Some(a), page size 2: both the page-1 and cursor-page
    // specialized variants execute; every page stays within the partner
    let mut collected = Vec::new();
    let mut after = None;
    let mut pages = 0;
    loop {
        let ret = contacts
            .list_for_filters(
                ContactScope::All,
                ContactFilters {
                    partner_id: Some(partner_a),
                    ..Default::default()
                },
                Sort {
                    by: ContactSortBy::CreatedAt,
                    direction: ListDirection::Descending,
                },
                PaginatedQueryArgs { first: 2, after },
            )
            .await?;
        pages += 1;
        assert!(ret.entities.iter().all(|c| c.partner_id == partner_a));
        collected.extend(ret.entities.iter().map(|c| c.id));
        if !ret.has_next_page {
            break;
        }
        after = ret.end_cursor;
    }
    assert!(pages >= 3, "expected pagination across pages, got {pages}");
    let expected: std::collections::HashSet<_> = ids_a.into_iter().collect();
    let collected: std::collections::HashSet<_> = collected.into_iter().collect();
    assert_eq!(collected, expected);

    Ok(())
}
