mod entities;
mod helpers;

use sqlx::PgPool;

use entities::facility::*;
use es_entity::*;

/// Two-dimension scoped repo mirroring lana's admin/partner/customer split:
/// `partner_id` and `customer_id` are both `scope` columns. Reads are scoped
/// by exactly one dimension (`PartnerId(_)`, `CustomerId(_)`) or by `All` —
/// never by both at once (disjunctive scoping).
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "Facility",
    columns(
        partner_id(ty = "PartnerId", scope),
        customer_id(ty = "CustomerId", scope),
        status(ty = "String", list_for(by(created_at))),
    )
)]
pub struct Facilities {
    pool: PgPool,
}

impl Facilities {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

async fn seed_facilities(
    repo: &Facilities,
    partner_id: PartnerId,
    customer_id: CustomerId,
    specs: &[&str],
) -> anyhow::Result<Vec<FacilityId>> {
    let mut ids = Vec::new();
    for status in specs {
        let id = FacilityId::new();
        let new = NewFacility::builder()
            .id(id)
            .partner_id(partner_id)
            .customer_id(customer_id)
            .status(*status)
            .build()
            .unwrap();
        // create is deliberately unscoped: both scope columns are ordinary
        // NewEntity data.
        repo.create(new).await?;
        ids.push(id);
    }
    Ok(ids)
}

#[tokio::test]
async fn multi_scoped_point_reads() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let facilities = Facilities::new(pool);

    let partner_a = PartnerId::new();
    let partner_b = PartnerId::new();
    let customer_x = CustomerId::new();
    let customer_y = CustomerId::new();

    let ids_ax = seed_facilities(&facilities, partner_a, customer_x, &["active"]).await?;
    let ids_by = seed_facilities(&facilities, partner_b, customer_y, &["active"]).await?;
    let (id_ax, id_by) = (ids_ax[0], ids_by[0]);

    // own dimension: raw ids route to the right variant via `From`
    let found = facilities.find_by_id(partner_a, id_ax).await?;
    assert_eq!(found.partner_id, partner_a);
    let found = facilities.find_by_id(customer_x, id_ax).await?;
    assert_eq!(found.customer_id, customer_x);

    // explicit enum forms
    facilities
        .find_by_id(FacilityScope::PartnerId(partner_a), id_ax)
        .await?;
    facilities
        .find_by_id(FacilityScope::CustomerId(customer_x), id_ax)
        .await?;

    // cross-dimension / foreign misses look identical to a missing row
    let err = facilities.find_by_id(partner_b, id_ax).await;
    assert!(matches!(err, Err(FacilityFindError::NotFound { .. })));
    assert!(
        facilities
            .maybe_find_by_id(customer_y, id_ax)
            .await?
            .is_none()
    );

    // All sees both rows
    facilities.find_by_id(FacilityScope::All, id_ax).await?;
    facilities.find_by_id(FacilityScope::All, id_by).await?;

    Ok(())
}

#[tokio::test]
async fn multi_scoped_find_all_intersects_per_dimension() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let facilities = Facilities::new(pool);

    let partner_a = PartnerId::new();
    let partner_b = PartnerId::new();
    let customer_x = CustomerId::new();
    let customer_y = CustomerId::new();

    let ids_ax = seed_facilities(&facilities, partner_a, customer_x, &["active", "active"]).await?;
    let ids_by = seed_facilities(&facilities, partner_b, customer_y, &["active"]).await?;

    let all_ids: Vec<FacilityId> = ids_ax.iter().chain(ids_by.iter()).copied().collect();

    // PartnerId(a): only that partner's rows — the other dimension's rows
    // are silently absent
    let found = facilities.find_all::<Facility>(partner_a, &all_ids).await?;
    assert_eq!(found.len(), 2);
    assert!(ids_ax.iter().all(|id| found.contains_key(id)));
    assert!(ids_by.iter().all(|id| !found.contains_key(id)));

    // CustomerId(y): only that customer's rows
    let found = facilities
        .find_all::<Facility>(customer_y, &all_ids)
        .await?;
    assert_eq!(found.len(), 1);
    assert!(found.contains_key(&ids_by[0]));

    // All: everything
    let found = facilities
        .find_all::<Facility>(FacilityScope::All, &all_ids)
        .await?;
    assert_eq!(found.len(), 3);

    Ok(())
}

#[tokio::test]
async fn multi_scoped_lists_paginate_per_dimension() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let facilities = Facilities::new(pool);

    let partner_a = PartnerId::new();
    let customer_x = CustomerId::new();
    let customer_y = CustomerId::new();

    // same partner, split across two customers
    let ids_x = seed_facilities(&facilities, partner_a, customer_x, &["active", "active"]).await?;
    let ids_y = seed_facilities(&facilities, partner_a, customer_y, &["active"]).await?;

    // PartnerId(a) sees all 3 rows, paginated
    let mut collected = Vec::new();
    let mut after: Option<facility_cursor::FacilityByCreatedAtCursor> = None;
    loop {
        let ret = facilities
            .list_by_created_at(
                partner_a,
                PaginatedQueryArgs { first: 2, after },
                ListDirection::Descending,
            )
            .await?;
        collected.extend(ret.entities.iter().map(|f| f.id));
        if !ret.has_next_page {
            break;
        }
        after = ret.end_cursor;
    }
    let expected: std::collections::HashSet<_> =
        ids_x.iter().chain(ids_y.iter()).copied().collect();
    let collected: std::collections::HashSet<_> = collected.into_iter().collect();
    assert_eq!(collected, expected);

    // CustomerId(x) sees only its own 2 rows
    let ret = facilities
        .list_by_created_at(
            customer_x,
            PaginatedQueryArgs {
                first: 100,
                after: None,
            },
            ListDirection::Descending,
        )
        .await?;
    assert_eq!(ret.entities.len(), 2);
    assert!(ret.entities.iter().all(|f| f.customer_id == customer_x));

    Ok(())
}

#[tokio::test]
async fn multi_scoped_reads_in_op() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let facilities = Facilities::new(pool);

    let partner_a = PartnerId::new();
    let partner_b = PartnerId::new();
    let customer_x = CustomerId::new();
    let ids = seed_facilities(&facilities, partner_a, customer_x, &["active"]).await?;

    let mut op = facilities.begin_op().await?;
    facilities
        .find_by_id_in_op(&mut op, partner_a, ids[0])
        .await?;
    facilities
        .find_by_id_in_op(&mut op, customer_x, ids[0])
        .await?;
    assert!(
        facilities
            .maybe_find_by_id_in_op(&mut op, partner_b, ids[0])
            .await?
            .is_none()
    );
    op.commit().await?;

    Ok(())
}

#[tokio::test]
async fn multi_scoped_bound_view() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let facilities = Facilities::new(pool);

    let partner_a = PartnerId::new();
    let customer_x = CustomerId::new();
    let customer_y = CustomerId::new();
    seed_facilities(&facilities, partner_a, customer_x, &["active"]).await?;
    seed_facilities(&facilities, partner_a, customer_y, &["active"]).await?;

    let view = facilities.scoped(customer_x);
    assert!(matches!(view.scope(), FacilityScope::CustomerId(c) if c == customer_x));

    let ret = view
        .list_by_created_at(
            PaginatedQueryArgs {
                first: 100,
                after: None,
            },
            ListDirection::Descending,
        )
        .await?;
    assert!(ret.entities.iter().all(|f| f.customer_id == customer_x));

    Ok(())
}

#[tokio::test]
async fn multi_scoped_filters_compose_per_dimension() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let facilities = Facilities::new(pool);

    let partner_a = PartnerId::new();
    let customer_x = CustomerId::new();
    seed_facilities(&facilities, partner_a, customer_x, &["active", "inactive"]).await?;

    let ret = facilities
        .list_for_status_by_created_at(
            customer_x,
            "active",
            PaginatedQueryArgs {
                first: 100,
                after: None,
            },
            ListDirection::Descending,
        )
        .await?;
    assert_eq!(ret.entities.len(), 1);
    assert_eq!(ret.entities[0].status, "active");
    assert_eq!(ret.entities[0].customer_id, customer_x);

    Ok(())
}

/// A cursor minted under one dimension replayed under another repositions
/// pagination but never widens the result set — every page's SQL carries the
/// scope conjunct for the dimension actually passed at call time.
#[tokio::test]
async fn cursor_replay_across_dimensions_never_widens() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let facilities = Facilities::new(pool);

    let partner_a = PartnerId::new();
    let customer_x = CustomerId::new();
    let customer_y = CustomerId::new();
    seed_facilities(&facilities, partner_a, customer_x, &["active", "active"]).await?;
    seed_facilities(&facilities, partner_a, customer_y, &["active"]).await?;

    // mint a cursor under PartnerId(a)
    let ret = facilities
        .list_by_created_at(
            partner_a,
            PaginatedQueryArgs {
                first: 1,
                after: None,
            },
            ListDirection::Descending,
        )
        .await?;
    let cursor = ret.end_cursor.expect("expected a next page");

    // replay it under CustomerId(x): every row returned must still belong to
    // customer_x — the foreign cursor repositions but cannot leak rows
    let ret = facilities
        .list_by_created_at(
            customer_x,
            PaginatedQueryArgs {
                first: 100,
                after: Some(cursor),
            },
            ListDirection::Descending,
        )
        .await?;
    assert!(ret.entities.iter().all(|f| f.customer_id == customer_x));

    Ok(())
}
