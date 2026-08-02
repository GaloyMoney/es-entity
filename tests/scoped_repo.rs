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
///
/// The scope column additionally opts into `find_by`/`list_for`: callers
/// filter by `partner_id` through the normal query surface, composing with
/// the scope — under `Only(a)` a mismatching caller value short-circuits to
/// an empty result, a matching one collapses into the scope predicate.
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "Contact",
    columns(
        partner_id(ty = "PartnerId", scope, find_by = true, list_for(by(created_at))),
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
                ..Default::default()
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

/// The `repo.scoped(scope)` bound view captures the scope once and exposes
/// every read fn without the per-call scope argument, delegating to the
/// scope-argument fns.
#[tokio::test]
async fn scoped_view_delegates() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let contacts = Contacts::new(pool);

    let partner_a = PartnerId::new();
    let partner_b = PartnerId::new();
    let ids_a = seed_contacts(
        &contacts,
        partner_a,
        &[("v1", "active"), ("v2", "inactive")],
    )
    .await?;
    let ids_b = seed_contacts(&contacts, partner_b, &[("v3", "active")]).await?;

    let view = contacts.scoped(partner_a);
    assert!(matches!(view.scope(), ContactScope::Only(p) if p == partner_a));

    // point reads — no scope argument at the call sites
    let contact = view.find_by_id(ids_a[0]).await?;
    assert_eq!(contact.partner_id, partner_a);
    view.find_by_email(&contact.email).await?;
    assert!(view.maybe_find_by_id(ids_b[0]).await?.is_none());
    assert!(matches!(
        view.find_by_id(ids_b[0]).await,
        Err(ContactFindError::NotFound { .. })
    ));

    // batch + lists
    let all_ids: Vec<ContactId> = ids_a.iter().chain(ids_b.iter()).copied().collect();
    let found = view.find_all::<Contact>(&all_ids).await?;
    assert_eq!(found.len(), 2);

    let page = view
        .list_by_created_at(
            PaginatedQueryArgs {
                first: 100,
                after: None,
            },
            ListDirection::Descending,
        )
        .await?;
    assert_eq!(page.entities.len(), 2);
    assert!(page.entities.iter().all(|c| c.partner_id == partner_a));

    let ret = view
        .list_for_status_by_created_at(
            "active",
            PaginatedQueryArgs {
                first: 100,
                after: None,
            },
            ListDirection::Descending,
        )
        .await?;
    assert_eq!(ret.entities.len(), 1);

    let ret = view
        .list_for_filters(
            ContactFilters {
                status: Some("inactive".to_string()),
                ..Default::default()
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

    // `_in_op` variants through the view
    let mut op = contacts.begin_op().await?;
    view.find_by_id_in_op(&mut op, ids_a[0]).await?;
    assert!(
        view.maybe_find_by_id_in_op(&mut op, ids_b[0])
            .await?
            .is_none()
    );
    op.commit().await?;

    // an All-bound view reads across scopes
    let all_view = contacts.scoped(ContactScope::All);
    all_view.find_by_id(ids_b[0]).await?;
    assert_eq!(all_view.find_all::<Contact>(&all_ids).await?.len(), 3);

    Ok(())
}

/// The scope column opted into `find_by = true`: the lookup composes with
/// the scope. Under `Only(a)` a lookup of `b != a` is a scope-guaranteed
/// miss — short-circuited in Rust without querying; `b == a` collapses into
/// the single scope predicate.
#[tokio::test]
async fn scope_column_find_by_composes() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let contacts = Contacts::new(pool);

    let partner_a = PartnerId::new();
    let partner_b = PartnerId::new();
    seed_contacts(&contacts, partner_a, &[("fb-a", "active")]).await?;
    seed_contacts(&contacts, partner_b, &[("fb-b", "active")]).await?;

    // All + value: plain lookup by the scope column
    let found = contacts
        .find_by_partner_id(ContactScope::All, partner_a)
        .await?;
    assert_eq!(found.partner_id, partner_a);

    // Only(a) + a: match — behaves like the scoped read
    let found = contacts.find_by_partner_id(partner_a, partner_a).await?;
    assert_eq!(found.partner_id, partner_a);

    // Only(a) + b: mismatch — not-found, indistinguishable from missing
    let err = contacts.find_by_partner_id(partner_a, partner_b).await;
    assert!(matches!(err, Err(ContactFindError::NotFound { .. })));
    assert!(
        contacts
            .maybe_find_by_partner_id(partner_a, partner_b)
            .await?
            .is_none()
    );

    Ok(())
}

/// The scope column opted into `list_for`: `ContactFilters` carries
/// `partner_id` and the predicates compose. The four scope × filter
/// combinations:
///
/// - `All` + `None`      → unfiltered
/// - `All` + `Some(p)`   → rows of `p`
/// - `Only(a)` + `None`  → rows of `a`
/// - `Only(a)` + `Some(b)` → empty unless `a == b` (mismatch short-circuits
///   without querying; a match collapses into the scope predicate)
#[tokio::test]
async fn scope_column_filter_combinations() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let contacts = Contacts::new(pool);

    let partner_a = PartnerId::new();
    let partner_b = PartnerId::new();
    let ids_a = seed_contacts(
        &contacts,
        partner_a,
        &[("c1", "active"), ("c2", "inactive")],
    )
    .await?;
    seed_contacts(&contacts, partner_b, &[("c3", "active")]).await?;

    let query = || PaginatedQueryArgs {
        first: 100,
        after: None,
    };
    let sort = || Sort {
        by: ContactSortBy::CreatedAt,
        direction: ListDirection::Descending,
    };
    let partner_filter = |p: PartnerId| ContactFilters {
        partner_id: Some(p),
        ..Default::default()
    };

    // All + None: unfiltered (other tests seed the shared table — check
    // containment, not count)
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

    // All + Some(a): the caller's partner choice narrows the listing
    let ret = contacts
        .list_for_filters(
            ContactScope::All,
            partner_filter(partner_a),
            sort(),
            query(),
        )
        .await?;
    assert_eq!(ret.entities.len(), 2);
    assert!(ret.entities.iter().all(|c| c.partner_id == partner_a));

    // Only(a) + None: the scope alone filters
    let ret = contacts
        .list_for_filters(partner_a, ContactFilters::default(), sort(), query())
        .await?;
    assert_eq!(ret.entities.len(), 2);

    // Only(a) + Some(a): match — same result as the scope alone
    let ret = contacts
        .list_for_filters(partner_a, partner_filter(partner_a), sort(), query())
        .await?;
    assert_eq!(ret.entities.len(), 2);

    // Only(a) + Some(b): a caller filter can never widen the scope — the
    // mismatch honestly returns nothing instead of being silently ignored
    let ret = contacts
        .list_for_filters(partner_a, partner_filter(partner_b), sort(), query())
        .await?;
    assert!(ret.entities.is_empty());
    assert!(!ret.has_next_page);
    assert!(ret.end_cursor.is_none());

    // dedicated single-filter fn composes the same way
    let ret = contacts
        .list_for_partner_id_by_created_at(
            ContactScope::All,
            partner_a,
            PaginatedQueryArgs {
                first: 100,
                after: None,
            },
            ListDirection::Descending,
        )
        .await?;
    assert_eq!(ret.entities.len(), 2);
    let ret = contacts
        .list_for_partner_id_by_created_at(
            partner_a,
            partner_b,
            PaginatedQueryArgs {
                first: 100,
                after: None,
            },
            ListDirection::Descending,
        )
        .await?;
    assert!(ret.entities.is_empty());

    // multi-filter (scope column + status) routes through the filters fn
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
    let ret = contacts
        .list_for_filters(
            ContactScope::All,
            ContactFilters {
                partner_id: Some(partner_a),
                status: Some("active".to_string()),
            },
            sort(),
            query(),
        )
        .await?;
    assert_eq!(ret.entities.len(), 1);
    assert_eq!(ret.entities[0].partner_id, partner_a);

    Ok(())
}

/// Cursor pagination respects the composed predicates: every page of an
/// `All` + partner-filtered listing stays within the filtered partner, and a
/// scope/filter mismatch with a cursor still short-circuits to an empty page.
#[tokio::test]
async fn scope_column_filter_cursor_pagination() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let contacts = Contacts::new(pool);

    let partner_a = PartnerId::new();
    let partner_b = PartnerId::new();
    let specs: Vec<(&str, &str)> = (0..5).map(|_| ("pg", "active")).collect();
    let ids_a = seed_contacts(&contacts, partner_a, &specs).await?;
    seed_contacts(&contacts, partner_b, &specs).await?;

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
        for entity in &ret.entities {
            assert_eq!(
                entity.partner_id, partner_a,
                "partner filter leaked a foreign row"
            );
        }
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

    // mismatch + cursor: still an empty page, no query needed
    let cursor_page = contacts
        .list_for_partner_id_by_created_at(
            ContactScope::All,
            partner_a,
            PaginatedQueryArgs {
                first: 1,
                after: None,
            },
            ListDirection::Descending,
        )
        .await?;
    let ret = contacts
        .list_for_partner_id_by_created_at(
            partner_b,
            partner_a,
            PaginatedQueryArgs {
                first: 100,
                after: cursor_page.end_cursor,
            },
            ListDirection::Descending,
        )
        .await?;
    assert!(ret.entities.is_empty());
    assert!(!ret.has_next_page);

    Ok(())
}
