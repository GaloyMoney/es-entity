mod entities;
mod helpers;

use std::time::Duration;

use entities::facility::*;
use es_entity::*;

pub mod unscoped {
    use sqlx::PgPool;

    use super::entities::facility::*;
    use es_entity::*;

    #[derive(EsRepo, Debug)]
    #[es_repo(
        entity = "Facility",
        columns(
            partner_id(ty = "PartnerId"),
            customer_id(ty = "CustomerId"),
            status(ty = "String", list_for(by(created_at))),
        )
    )]
    pub struct UnscopedFacilities {
        pool: PgPool,
    }

    impl UnscopedFacilities {
        pub fn new(pool: PgPool) -> Self {
            Self { pool }
        }
    }
}

pub mod scoped {
    use sqlx::PgPool;

    use super::entities::facility::*;
    use es_entity::*;

    #[derive(EsRepo, Debug)]
    #[es_repo(
        entity = "Facility",
        columns(
            partner_id(ty = "PartnerId", scope),
            customer_id(ty = "CustomerId"),
            status(ty = "String", list_for(by(created_at))),
        )
    )]
    pub struct ScopedFacilities {
        pool: PgPool,
    }

    impl ScopedFacilities {
        pub fn new(pool: PgPool) -> Self {
            Self { pool }
        }
    }
}

fn unique_status(label: &str) -> String {
    format!("{label}-{}", FacilityId::new())
}

fn new_facility(partner_id: PartnerId, status: &str) -> NewFacility {
    NewFacility::builder()
        .partner_id(partner_id)
        .customer_id(CustomerId::new())
        .status(status)
        .build()
}

async fn tick() {
    tokio::time::sleep(Duration::from_millis(50)).await;
}

fn ids(facilities: &[Facility]) -> Vec<FacilityId> {
    facilities.iter().map(|f| f.id).collect()
}

#[tokio::test]
async fn groups_rows_per_key_in_sort_order_and_omits_absent_keys() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = unscoped::UnscopedFacilities::new(pool);
    let partner = PartnerId::new();
    let (a, b, c) = (unique_status("a"), unique_status("b"), unique_status("c"));

    let a1 = repo.create(new_facility(partner, &a)).await?.id;
    tick().await;
    let b1 = repo.create(new_facility(partner, &b)).await?.id;
    tick().await;
    let a2 = repo.create(new_facility(partner, &a)).await?.id;

    let keys = vec![a.clone(), b.clone(), c.clone()];

    let desc: std::collections::HashMap<String, Vec<Facility>> = repo
        .find_all_for_statuses(&keys, ListDirection::Descending)
        .await?;
    assert_eq!(desc.len(), 2);
    assert_eq!(ids(&desc[&a]), vec![a2, a1]);
    assert_eq!(ids(&desc[&b]), vec![b1]);
    assert!(!desc.contains_key(&c));

    let asc: std::collections::HashMap<String, Vec<Facility>> = repo
        .find_all_for_statuses(&keys, ListDirection::Ascending)
        .await?;
    assert_eq!(asc.len(), 2);
    assert_eq!(ids(&asc[&a]), vec![a1, a2]);
    assert_eq!(ids(&asc[&b]), vec![b1]);
    assert!(!asc.contains_key(&c));

    Ok(())
}

#[tokio::test]
async fn identical_created_at_falls_back_to_id() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = unscoped::UnscopedFacilities::new(pool);
    let partner = PartnerId::new();
    let a = unique_status("tie");

    let created = repo
        .create_all(vec![
            new_facility(partner, &a),
            new_facility(partner, &a),
            new_facility(partner, &a),
        ])
        .await?;
    let mut by_id: Vec<FacilityId> = created.iter().map(|f| f.id).collect();
    by_id.sort();

    let keys = [a.clone()];
    let asc: std::collections::HashMap<String, Vec<Facility>> = repo
        .find_all_for_statuses(&keys, ListDirection::Ascending)
        .await?;
    assert_eq!(ids(&asc[&a]), by_id);

    let desc: std::collections::HashMap<String, Vec<Facility>> = repo
        .find_all_for_statuses(&keys, ListDirection::Descending)
        .await?;
    let mut by_id_desc = by_id.clone();
    by_id_desc.reverse();
    assert_eq!(ids(&desc[&a]), by_id_desc);

    Ok(())
}

#[tokio::test]
async fn interleaved_inserts_still_group_per_key() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = unscoped::UnscopedFacilities::new(pool);
    let partner = PartnerId::new();
    let keys: Vec<String> = ["x", "y", "z"].iter().map(|k| unique_status(k)).collect();

    let mut created: Vec<Vec<FacilityId>> = vec![vec![], vec![], vec![]];
    for _ in 0..3 {
        for (i, key) in keys.iter().enumerate() {
            created[i].push(repo.create(new_facility(partner, key)).await?.id);
            tick().await;
        }
    }

    let asc: std::collections::HashMap<String, Vec<Facility>> = repo
        .find_all_for_statuses(&keys, ListDirection::Ascending)
        .await?;
    let desc: std::collections::HashMap<String, Vec<Facility>> = repo
        .find_all_for_statuses(&keys, ListDirection::Descending)
        .await?;
    for (i, key) in keys.iter().enumerate() {
        assert_eq!(ids(&asc[key]), created[i]);
        let mut reversed = created[i].clone();
        reversed.reverse();
        assert_eq!(ids(&desc[key]), reversed);
    }

    Ok(())
}

#[tokio::test]
async fn empty_key_slice_gives_empty_map() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = unscoped::UnscopedFacilities::new(pool);

    let found: std::collections::HashMap<String, Vec<Facility>> = repo
        .find_all_for_statuses(&[], ListDirection::Descending)
        .await?;
    assert!(found.is_empty());

    Ok(())
}

#[tokio::test]
async fn scope_restricts_rows_for_the_same_key_list() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = scoped::ScopedFacilities::new(pool);
    let (partner_x, partner_y) = (PartnerId::new(), PartnerId::new());
    let a = unique_status("a");
    let b = unique_status("b");

    let ax = repo.create(new_facility(partner_x, &a)).await?.id;
    tick().await;
    let ay = repo.create(new_facility(partner_y, &a)).await?.id;
    tick().await;
    let bx = repo.create(new_facility(partner_x, &b)).await?.id;
    let keys = vec![a.clone(), b.clone()];

    let only_x: std::collections::HashMap<String, Vec<Facility>> = repo
        .find_all_for_statuses(partner_x, &keys, ListDirection::Descending)
        .await?;
    assert_eq!(only_x.len(), 2);
    assert_eq!(ids(&only_x[&a]), vec![ax]);
    assert_eq!(ids(&only_x[&b]), vec![bx]);

    let only_y: std::collections::HashMap<String, Vec<Facility>> = repo
        .find_all_for_statuses(partner_y, &keys, ListDirection::Descending)
        .await?;
    assert_eq!(only_y.len(), 1);
    assert_eq!(ids(&only_y[&a]), vec![ay]);

    let all: std::collections::HashMap<String, Vec<Facility>> = repo
        .find_all_for_statuses(scoped::FacilityScope::All, &keys, ListDirection::Descending)
        .await?;
    assert_eq!(ids(&all[&a]), vec![ay, ax]);
    assert_eq!(ids(&all[&b]), vec![bx]);

    let bound: std::collections::HashMap<String, Vec<Facility>> = repo
        .scoped(partner_y)
        .find_all_for_statuses(&keys, ListDirection::Ascending)
        .await?;
    assert_eq!(bound.len(), 1);
    assert_eq!(ids(&bound[&a]), vec![ay]);

    Ok(())
}
