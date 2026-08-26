mod entities;
mod helpers;

use sqlx::PgPool;

use entities::party::*;
use es_entity::*;

/// Mirrors lana's `core/party` shape: `customer_id` is a **nullable** scope
/// column (`Option<CustomerId>`). The generated `PartyScope::Customer`
/// variant carries the bare `CustomerId`, never `Option<CustomerId>` — a
/// scope value is always concrete. Rows whose `customer_id` is NULL (no
/// customer owns them directly) are invisible to every `Customer(_)` scoped
/// read; only `All` sees them.
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "Party",
    columns(
        customer_id(
            ty = "Option<CustomerId>",
            scope(variant = "Customer"),
            update(persist = false)
        ),
        name(ty = "String"),
    )
)]
pub struct Parties {
    pool: PgPool,
}

impl Parties {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

async fn seed_owned(
    repo: &Parties,
    customer_id: CustomerId,
    name: &str,
) -> anyhow::Result<PartyId> {
    let id = PartyId::new();
    let new = NewParty::builder()
        .id(id)
        .customer_id(customer_id)
        .name(name)
        .build()
        .unwrap();
    repo.create(new).await?;
    Ok(id)
}

async fn seed_unowned(repo: &Parties, name: &str) -> anyhow::Result<PartyId> {
    let id = PartyId::new();
    // No `.customer_id(..)` call — the field stays `None`, persisted as SQL
    // NULL: this row belongs to no customer directly.
    let new = NewParty::builder().id(id).name(name).build().unwrap();
    repo.create(new).await?;
    Ok(id)
}

/// The generated variant carries the inner `CustomerId`, not
/// `Option<CustomerId>` — this is a compile-time property (the line below
/// would not type-check if the variant were `Customer(Option<CustomerId>)`
/// or if a `From<Option<CustomerId>>` impl existed), asserted here so it's
/// exercised on every test run.
#[tokio::test]
async fn scope_variant_carries_inner_type_not_option() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let parties = Parties::new(pool);

    let customer_id = CustomerId::new();
    let id = seed_owned(&parties, customer_id, "acme individual").await?;

    // raw CustomerId routes through `From<CustomerId> for PartyScope`
    let found = parties.find_by_id(customer_id, id).await?;
    assert_eq!(found.customer_id, Some(customer_id));

    // explicit enum spelling: `PartyScope::Customer` takes `CustomerId`
    // directly.
    let scope = PartyScope::Customer(customer_id);
    parties.find_by_id(scope, id).await?;

    Ok(())
}

/// The defining nullable-scope semantics: a NULL-`customer_id` row is
/// invisible under `Customer(_)` (looks exactly like a missing / foreign row)
/// but visible under `All`.
#[tokio::test]
async fn null_scoped_row_invisible_to_customer_scope_visible_to_all() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let parties = Parties::new(pool);

    let customer_id = CustomerId::new();
    let unowned_id = seed_unowned(&parties, "org member individual").await?;

    // NOT NotFound because of a real customer mismatch — NotFound because
    // the row's customer_id is NULL and `customer_id = $1` never matches
    // NULL, for any bound value.
    let err = parties.find_by_id(customer_id, unowned_id).await;
    assert!(matches!(err, Err(PartyFindError::NotFound { .. })));
    assert!(
        parties
            .maybe_find_by_id(customer_id, unowned_id)
            .await?
            .is_none()
    );

    // a *different* customer scope value must fail identically — this is
    // not "wrong id", it's "no id could ever match".
    let other_customer_id = CustomerId::new();
    assert!(
        parties
            .maybe_find_by_id(other_customer_id, unowned_id)
            .await?
            .is_none()
    );

    // `All` is the audited escape hatch: it still sees the row.
    let found = parties.find_by_id(PartyScope::All, unowned_id).await?;
    assert_eq!(found.customer_id, None);

    Ok(())
}

/// `find_all` under a customer scope must silently drop NULL-scoped rows
/// from a batch that mixes owned and unowned ids — never surface them, never
/// error.
#[tokio::test]
async fn find_all_excludes_null_scoped_rows_under_customer_scope() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let parties = Parties::new(pool);

    let customer_id = CustomerId::new();
    let owned_id = seed_owned(&parties, customer_id, "owned").await?;
    let unowned_id = seed_unowned(&parties, "unowned").await?;
    let ids = vec![owned_id, unowned_id];

    let found = parties.find_all::<Party>(customer_id, &ids).await?;
    assert_eq!(found.len(), 1);
    assert!(found.contains_key(&owned_id));
    assert!(!found.contains_key(&unowned_id));

    let found = parties.find_all::<Party>(PartyScope::All, &ids).await?;
    assert_eq!(found.len(), 2);

    Ok(())
}

/// `list_by_created_at` under a customer scope must never page in a
/// NULL-scoped row — checked across the full paginated walk, not just a
/// first page, since the nullable-aware `NULLS FIRST/LAST` cursor machinery
/// used by ordinary (non-scope) nullable columns must not leak into the
/// scope predicate itself.
#[tokio::test]
async fn list_excludes_null_scoped_rows_across_pagination() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let parties = Parties::new(pool);

    let customer_id = CustomerId::new();
    let owned_a = seed_owned(&parties, customer_id, "owned_a").await?;
    let owned_b = seed_owned(&parties, customer_id, "owned_b").await?;
    let _unowned = seed_unowned(&parties, "unowned").await?;
    let _other_customer = seed_owned(&parties, CustomerId::new(), "other customer").await?;

    let mut collected = Vec::new();
    let mut after: Option<party_cursor::PartyByCreatedAtCursor> = None;
    loop {
        let ret = parties
            .list_by_created_at(
                customer_id,
                PaginatedQueryArgs { first: 1, after },
                ListDirection::Descending,
            )
            .await?;
        collected.extend(ret.entities.iter().map(|p| p.id));
        if !ret.has_next_page {
            break;
        }
        after = ret.end_cursor;
    }

    let expected: std::collections::HashSet<_> = [owned_a, owned_b].into_iter().collect();
    let collected: std::collections::HashSet<_> = collected.into_iter().collect();
    assert_eq!(collected, expected);

    Ok(())
}

/// `repo.scoped(customer_id)` — the bound view — inherits the same
/// NULL-exclusion behavior as the scope-argument fns it delegates to.
#[tokio::test]
async fn bound_view_excludes_null_scoped_rows() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let parties = Parties::new(pool);

    let customer_id = CustomerId::new();
    let owned_id = seed_owned(&parties, customer_id, "owned").await?;
    let _unowned_id = seed_unowned(&parties, "unowned").await?;

    let view = parties.scoped(customer_id);
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
    assert_eq!(ret.entities[0].id, owned_id);

    Ok(())
}
