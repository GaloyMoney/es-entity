mod entities;
mod helpers;

use sqlx::PgPool;

use entities::contact::*;
use es_entity::*;

/// Partner-scoped repo: every generated read fn requires a leading
/// `scope: impl Into<ContactScope>` argument. `Only(partner_id)` filters all
/// reads by the scope column; `All` reads across scopes. Writes (`create`,
/// `update`, `delete`) keep their unscoped signatures — mutations operate on
/// entities that could only have been obtained through a scoped read.
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "Contact",
    columns(
        partner_id(ty = "PartnerId", scope),
        email(ty = "String"),
        status(ty = "String", list_for(by(created_at))),
    )
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
        // create is deliberately unscoped: the scope column is ordinary
        // NewEntity data.
        repo.create(new).await?;
        ids.push(id);
    }
    Ok(ids)
}

#[tokio::test]
async fn scoped_point_reads() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let contacts = Contacts::new(pool);

    let partner_a = PartnerId::new();
    let partner_b = PartnerId::new();
    let ids_a = seed_contacts(&contacts, partner_a, &[("a", "active")]).await?;
    let ids_b = seed_contacts(&contacts, partner_b, &[("b", "active")]).await?;
    let (id_a, id_b) = (ids_a[0], ids_b[0]);

    // own scope: found — `impl Into<ContactScope>` accepts the partner id
    // directly (From<PartnerId> => Only), a reference, or the explicit enum.
    let found = contacts.find_by_id(partner_a, id_a).await?;
    assert_eq!(found.partner_id, partner_a);
    contacts.find_by_id(&partner_a, id_a).await?;
    contacts
        .find_by_id(ContactScope::Only(partner_a), id_a)
        .await?;

    // foreign scope: missing and not-yours look identical
    let err = contacts.find_by_id(partner_b, id_a).await;
    assert!(matches!(err, Err(ContactFindError::NotFound { .. })));
    assert!(contacts.maybe_find_by_id(partner_b, id_a).await?.is_none());

    // All: reads across scopes (audited escape hatch)
    contacts.find_by_id(ContactScope::All, id_a).await?;
    contacts.find_by_id(ContactScope::All, id_b).await?;

    // unique-column lookup is scoped the same way
    let email = contacts.find_by_id(partner_a, id_a).await?.email;
    contacts.find_by_email(partner_a, &email).await?;
    assert!(
        contacts
            .maybe_find_by_email(partner_b, &email)
            .await?
            .is_none()
    );

    Ok(())
}

#[tokio::test]
async fn scoped_reads_in_op() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let contacts = Contacts::new(pool);

    let partner_a = PartnerId::new();
    let partner_b = PartnerId::new();
    let ids = seed_contacts(&contacts, partner_a, &[("op", "active")]).await?;

    let mut op = contacts.begin_op().await?;
    contacts
        .find_by_id_in_op(&mut op, partner_a, ids[0])
        .await?;
    assert!(
        contacts
            .maybe_find_by_id_in_op(&mut op, partner_b, ids[0])
            .await?
            .is_none()
    );
    op.commit().await?;

    Ok(())
}

#[tokio::test]
async fn scoped_find_all_drops_foreign_ids() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let contacts = Contacts::new(pool);

    let partner_a = PartnerId::new();
    let partner_b = PartnerId::new();
    let ids_a = seed_contacts(
        &contacts,
        partner_a,
        &[("fa1", "active"), ("fa2", "active")],
    )
    .await?;
    let ids_b = seed_contacts(&contacts, partner_b, &[("fb1", "active")]).await?;

    let all_ids: Vec<ContactId> = ids_a.iter().chain(ids_b.iter()).copied().collect();

    // Only(a): foreign ids are silently absent — missing and not-yours look
    // identical.
    let found = contacts.find_all::<Contact>(partner_a, &all_ids).await?;
    assert_eq!(found.len(), 2);
    assert!(ids_a.iter().all(|id| found.contains_key(id)));
    assert!(ids_b.iter().all(|id| !found.contains_key(id)));

    // All: everything.
    let found = contacts
        .find_all::<Contact>(ContactScope::All, &all_ids)
        .await?;
    assert_eq!(found.len(), 3);

    Ok(())
}

#[tokio::test]
async fn scoped_list_by_paginates_within_scope() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let contacts = Contacts::new(pool);

    let partner_a = PartnerId::new();
    let partner_b = PartnerId::new();
    let specs: Vec<(&str, &str)> = (0..5).map(|_| ("list", "active")).collect();
    let ids_a = seed_contacts(&contacts, partner_a, &specs).await?;
    seed_contacts(&contacts, partner_b, &specs).await?;

    // paginate under Only(a) with a page size that forces both the page-1
    // and the cursor-page specialized variants to execute
    let mut collected = Vec::new();
    let mut after: Option<contact_cursor::ContactByCreatedAtCursor> = None;
    let mut pages = 0;
    loop {
        let ret = contacts
            .list_by_created_at(
                partner_a,
                PaginatedQueryArgs { first: 2, after },
                ListDirection::Descending,
            )
            .await?;
        pages += 1;
        for entity in &ret.entities {
            assert_eq!(
                entity.partner_id, partner_a,
                "scoped list leaked a foreign row"
            );
        }
        collected.extend(ret.entities.iter().map(|c| c.id));
        if !ret.has_next_page {
            break;
        }
        after = ret.end_cursor;
    }
    assert!(pages >= 3, "expected pagination across pages, got {pages}");
    assert_eq!(collected.len(), 5);
    let expected: std::collections::HashSet<_> = ids_a.into_iter().collect();
    let collected: std::collections::HashSet<_> = collected.into_iter().collect();
    assert_eq!(collected, expected);

    Ok(())
}

#[tokio::test]
async fn scoped_list_for_and_filters_dispatch() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let contacts = Contacts::new(pool);

    let partner_a = PartnerId::new();
    let partner_b = PartnerId::new();
    seed_contacts(
        &contacts,
        partner_a,
        &[("d1", "active"), ("d2", "inactive"), ("d3", "active")],
    )
    .await?;
    seed_contacts(&contacts, partner_b, &[("d4", "active")]).await?;

    // dedicated single-filter path
    let ret = contacts
        .list_for_status_by_created_at(
            partner_a,
            "active",
            PaginatedQueryArgs {
                first: 100,
                after: None,
            },
            ListDirection::Descending,
        )
        .await?;
    assert!(ret.entities.iter().all(|c| c.partner_id == partner_a));
    assert!(ret.entities.iter().all(|c| c.status == "active"));
    assert_eq!(ret.entities.len(), 2);

    // unified dispatch: no filter routes through the (scoped) list_by proxy
    let ret = contacts
        .list_for_filters(
            partner_a,
            ContactFilters::default(),
            Sort {
                by: ContactSortBy::CreatedAt,
                direction: ListDirection::Descending,
            },
            PaginatedQueryArgs {
                first: 100,
                after: None,
            },
        )
        .await?;
    assert_eq!(ret.entities.len(), 3);
    assert!(ret.entities.iter().all(|c| c.partner_id == partner_a));

    // unified dispatch: status filter routes through the (scoped) list_for proxy
    let ret = contacts
        .list_for_filters(
            partner_a,
            ContactFilters {
                status: Some("inactive".to_string()),
            },
            Sort {
                by: ContactSortBy::CreatedAt,
                direction: ListDirection::Descending,
            },
            PaginatedQueryArgs {
                first: 100,
                after: None,
            },
        )
        .await?;
    assert_eq!(ret.entities.len(), 1);
    assert_eq!(ret.entities[0].partner_id, partner_a);

    // All sees both partners' rows (restricted to this test's seeds via status)
    let ret = contacts
        .list_for_status_by_created_at(
            ContactScope::All,
            "inactive",
            PaginatedQueryArgs {
                first: 100,
                after: None,
            },
            ListDirection::Descending,
        )
        .await?;
    assert!(ret.entities.iter().any(|c| c.partner_id == partner_a));

    Ok(())
}

/// A cursor minted under one scope replayed under another repositions the
/// pagination but can never leak foreign rows — every page's SQL carries the
/// scope conjunct.
#[tokio::test]
async fn foreign_cursor_cannot_leak_rows() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let contacts = Contacts::new(pool);

    let partner_a = PartnerId::new();
    let partner_b = PartnerId::new();
    let specs: Vec<(&str, &str)> = (0..3).map(|_| ("cur", "active")).collect();
    seed_contacts(&contacts, partner_a, &specs).await?;
    seed_contacts(&contacts, partner_b, &specs).await?;

    let page_a = contacts
        .list_by_created_at(
            partner_a,
            PaginatedQueryArgs {
                first: 1,
                after: None,
            },
            ListDirection::Descending,
        )
        .await?;
    let cursor_from_a = page_a.end_cursor;

    let ret = contacts
        .list_by_created_at(
            partner_b,
            PaginatedQueryArgs {
                first: 100,
                after: cursor_from_a,
            },
            ListDirection::Descending,
        )
        .await?;
    assert!(
        ret.entities.iter().all(|c| c.partner_id == partner_b),
        "foreign cursor must not leak rows from another scope"
    );

    Ok(())
}

/// The `Only` arm's page-1 list SQL must be sargable against a
/// `(partner_id, created_at, id)` composite index — the scope conjunct is a
/// plain equality, not a COALESCE catch-all.
#[tokio::test]
async fn scoped_list_query_plan_uses_composite_index() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;

    let mut tx = pool.begin().await?;
    sqlx::query("SET LOCAL enable_seqscan = off")
        .execute(&mut *tx)
        .await?;
    // The exact page-1 `Only` arm SQL shape generated for
    // `list_by_created_at`.
    let rows: Vec<(String,)> = sqlx::query_as(
        "EXPLAIN SELECT created_at, id FROM contacts WHERE partner_id = $1 \
         ORDER BY created_at DESC, id DESC LIMIT $2",
    )
    .bind(uuid::Uuid::from(PartnerId::new()))
    .bind(5i64)
    .fetch_all(&mut *tx)
    .await?;
    tx.rollback().await?;

    let plan = rows
        .into_iter()
        .map(|(l,)| l)
        .collect::<Vec<_>>()
        .join("\n");
    // The exact index the planner picks depends on table statistics; the
    // property under test is sargability — the scope conjunct must become an
    // index qual, never a filter over a seq scan.
    assert!(
        plan.contains("Index Cond: (partner_id ="),
        "expected an index qual on partner_id, got plan:\n{plan}"
    );
    assert!(
        !plan.contains("Seq Scan"),
        "scope conjunct must not force a sequential scan, got plan:\n{plan}"
    );

    Ok(())
}
