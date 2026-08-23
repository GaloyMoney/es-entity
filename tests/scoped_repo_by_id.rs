mod entities;
mod helpers;

use sqlx::PgPool;

use entities::partner::*;
use es_entity::*;

/// Id-scoped repo — the tenancy-root variant of a scoped repository. The
/// entity has no separate tenant column: its rows' scope value is its own
/// `id`, marked via `id(scope)`. Every generated read fn requires a leading
/// `scope: impl Into<PartnerScope>` argument; under `Id(id)` every query
/// carries an `id = $n` conjunct, so reads collapse to self-or-nothing —
/// missing and not-yours look identical. `All` reads across scopes. Writes
/// (`create`, `update`, `delete`) keep their unscoped signatures.
#[derive(EsRepo, Debug)]
#[es_repo(entity = "Partner", columns(id(scope), name(ty = "String", list_by)))]
pub struct Partners {
    pool: PgPool,
}

impl Partners {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

async fn seed_partner(repo: &Partners, name: &str) -> anyhow::Result<PartnerId> {
    let id = PartnerId::new();
    let new = NewPartner::builder()
        .id(id)
        .name(format!("{name}-{id}"))
        .build()
        .unwrap();
    // create is deliberately unscoped: the id is ordinary NewEntity data.
    repo.create(new).await?;
    Ok(id)
}

#[tokio::test]
async fn id_scoped_point_reads() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let partners = Partners::new(pool);

    let partner_a = seed_partner(&partners, "a").await?;
    let partner_b = seed_partner(&partners, "b").await?;

    // own scope: found — `impl Into<PartnerScope>` accepts the id directly
    // (From<PartnerId> => Id), a reference, or the explicit enum.
    let found = partners.find_by_id(partner_a, partner_a).await?;
    assert_eq!(found.id, partner_a);
    partners.find_by_id(&partner_a, partner_a).await?;
    partners
        .find_by_id(PartnerScope::Id(partner_a), partner_a)
        .await?;

    // foreign scope: missing and not-yours look identical
    let err = partners.find_by_id(partner_b, partner_a).await;
    assert!(matches!(err, Err(PartnerFindError::NotFound { .. })));
    assert!(
        partners
            .maybe_find_by_id(partner_a, partner_b)
            .await?
            .is_none()
    );

    // All: reads across scopes (audited escape hatch)
    partners.find_by_id(PartnerScope::All, partner_a).await?;
    partners.find_by_id(PartnerScope::All, partner_b).await?;

    // lookup columns are scoped the same way: another partner's name is
    // invisible under your scope
    let name = partners.find_by_id(partner_a, partner_a).await?.name;
    partners.find_by_name(partner_a, &name).await?;
    assert!(
        partners
            .maybe_find_by_name(partner_b, &name)
            .await?
            .is_none()
    );

    Ok(())
}

#[tokio::test]
async fn id_scoped_reads_in_op() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let partners = Partners::new(pool);

    let partner_a = seed_partner(&partners, "op-a").await?;
    let partner_b = seed_partner(&partners, "op-b").await?;

    let mut op = partners.begin_op().await?;
    partners
        .find_by_id_in_op(&mut op, partner_a, partner_a)
        .await?;
    assert!(
        partners
            .maybe_find_by_id_in_op(&mut op, partner_b, partner_a)
            .await?
            .is_none()
    );
    op.commit().await?;

    Ok(())
}

#[tokio::test]
async fn id_scoped_find_all_intersects_to_own_row() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let partners = Partners::new(pool);

    let partner_a = seed_partner(&partners, "fa").await?;
    let partner_b = seed_partner(&partners, "fb").await?;
    let all_ids = [partner_a, partner_b];

    // Id(a): at most the own row — foreign ids are silently absent.
    let found = partners.find_all::<Partner>(partner_a, &all_ids).await?;
    assert_eq!(found.len(), 1);
    assert!(found.contains_key(&partner_a));

    // All: everything.
    let found = partners
        .find_all::<Partner>(PartnerScope::All, &all_ids)
        .await?;
    assert_eq!(found.len(), 2);

    Ok(())
}

#[tokio::test]
async fn id_scoped_lists_return_own_row() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let partners = Partners::new(pool);

    let partner_a = seed_partner(&partners, "la").await?;
    let partner_b = seed_partner(&partners, "lb").await?;

    // Id(a): exactly the own row, whatever the page size.
    let ret = partners
        .list_by_created_at(
            partner_a,
            PaginatedQueryArgs {
                first: 100,
                after: None,
            },
            ListDirection::Descending,
        )
        .await?;
    assert_eq!(ret.entities.len(), 1);
    assert_eq!(ret.entities[0].id, partner_a);
    assert!(!ret.has_next_page);

    // unified dispatch: no filter routes through the (scoped) list_by proxy
    let ret = partners
        .list_for_filters(
            partner_a,
            PartnerFilters::default(),
            Sort {
                by: PartnerSortBy::CreatedAt,
                direction: ListDirection::Descending,
            },
            PaginatedQueryArgs {
                first: 100,
                after: None,
            },
        )
        .await?;
    assert_eq!(ret.entities.len(), 1);
    assert_eq!(ret.entities[0].id, partner_a);

    // All sees both rows (fresh seeds are newest under Descending).
    let ret = partners
        .list_by_created_at(
            PartnerScope::All,
            PaginatedQueryArgs {
                first: 100,
                after: None,
            },
            ListDirection::Descending,
        )
        .await?;
    assert!(ret.entities.iter().any(|p| p.id == partner_a));
    assert!(ret.entities.iter().any(|p| p.id == partner_b));

    Ok(())
}

/// The `repo.scoped(scope)` bound view captures the scope once and exposes
/// every read fn without the per-call scope argument.
#[tokio::test]
async fn id_scoped_view_delegates() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let partners = Partners::new(pool);

    let partner_a = seed_partner(&partners, "va").await?;
    let partner_b = seed_partner(&partners, "vb").await?;

    let view = partners.scoped(partner_a);
    view.find_by_id(partner_a).await?;
    assert!(view.maybe_find_by_id(partner_b).await?.is_none());

    let found = view.find_all::<Partner>(&[partner_a, partner_b]).await?;
    assert_eq!(found.len(), 1);

    let ret = view
        .list_by_created_at(
            PaginatedQueryArgs {
                first: 100,
                after: None,
            },
            ListDirection::Descending,
        )
        .await?;
    assert_eq!(ret.entities.len(), 1);

    // `_in_op` variants through the view
    let mut op = partners.begin_op().await?;
    view.find_by_id_in_op(&mut op, partner_a).await?;
    assert!(
        view.maybe_find_by_id_in_op(&mut op, partner_b)
            .await?
            .is_none()
    );
    op.commit().await?;

    // All through the view
    let all_view = partners.scoped(PartnerScope::All);
    assert_eq!(
        all_view
            .find_all::<Partner>(&[partner_a, partner_b])
            .await?
            .len(),
        2
    );

    Ok(())
}

/// Same table as [`Partners`], but the tenancy-root variant is renamed via
/// `id(scope(variant = "Tenant"))` — proves the override reaches the
/// macro-owned `id` column, not just ordinary scope columns. Wrapped in its
/// own module: both repos share `entity = "Partner"`, so their
/// derive-generated companion types (`PartnerScope`, `PartnerFindError`, ...)
/// would otherwise collide.
pub mod overridden_id_scope {
    use sqlx::PgPool;

    use super::entities::partner::*;
    use es_entity::*;

    #[derive(EsRepo, Debug)]
    #[es_repo(
        entity = "Partner",
        columns(id(scope(variant = "Tenant")), name(ty = "String"))
    )]
    pub struct PartnersByOverriddenScope {
        pool: PgPool,
    }

    impl PartnersByOverriddenScope {
        pub fn new(pool: PgPool) -> Self {
            Self { pool }
        }
    }
}

#[tokio::test]
async fn id_scope_variant_override_dispatches_correctly() -> anyhow::Result<()> {
    use overridden_id_scope::*;

    let pool = helpers::init_pool().await?;
    let partners = PartnersByOverriddenScope::new(pool);

    let id = PartnerId::new();
    let new = NewPartner::builder()
        .id(id)
        .name(format!("tenant-{id}"))
        .build()
        .unwrap();
    partners.create(new).await?;

    // raw id routes through `From<PartnerId> for PartnerScope` into the
    // overridden `Tenant` variant
    let found = partners.find_by_id(id, id).await?;
    assert_eq!(found.id, id);

    // explicit overridden-variant spelling
    partners.find_by_id(PartnerScope::Tenant(id), id).await?;

    // a foreign id under the overridden variant still collapses to
    // self-or-nothing, same as the un-overridden `Id` variant does
    let foreign = PartnerId::new();
    assert!(matches!(
        partners.find_by_id(foreign, id).await,
        Err(PartnerFindError::NotFound { .. })
    ));

    // All still reads across scopes
    partners.find_by_id(PartnerScope::All, id).await?;

    Ok(())
}
