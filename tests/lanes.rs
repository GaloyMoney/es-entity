//! Nested repository rejection composition and partial domain mapping.
mod helpers;
use es_entity::*;
use repo_composition_source::*;

/// Maps the imported child leaf directly into a domain outcome.
#[derive(Debug, Clone, errlanes::Rejection, errlanes::Lift)]
#[lift(LaneParentConstraintViolation, unhandled = fatal)]
enum LaneOrderRejection {
    #[lift(LaneParentConstraintViolation::ItemsSkuKey)]
    DuplicateSku(ConstraintConflict<String>),
}

/// An explicit partial boundary that accepts none of the repository cases.
#[allow(dead_code)]
#[derive(Debug, Clone, errlanes::Rejection, errlanes::Lift)]
#[lift(LaneParentConstraintViolation, unhandled = fatal)]
enum NothingHoisted {
    Placeholder,
}

fn rejected(
    err: es_entity::RepoWriteError<LaneParentConstraintViolation>,
) -> LaneParentConstraintViolation {
    match err {
        Fail::Rejected(cv) => cv,
        other => panic!("expected Rejected, got {other:?}"),
    }
}

#[tokio::test]
async fn duplicate_nested_sku_hoists_through_the_parent_path() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let parents = LaneParents::new(pool);

    let sku = format!("sku-{}", LaneParentId::new());

    let mut first = parents
        .create(
            NewLaneParent::builder()
                .id(LaneParentId::new())
                .build()
                .unwrap(),
        )
        .await?;
    first.add_item(
        NewLaneItem::builder()
            .id(LaneItemId::new())
            .parent_id(first.id)
            .sku(sku.clone())
            .build()
            .unwrap(),
    );
    parents.update(&mut first).await?;

    let mut second = parents
        .create(
            NewLaneParent::builder()
                .id(LaneParentId::new())
                .build()
                .unwrap(),
        )
        .await?;
    second.add_item(
        NewLaneItem::builder()
            .id(LaneItemId::new())
            .parent_id(second.id)
            .sku(sku.clone())
            .build()
            .unwrap(),
    );
    let err = parents
        .update(&mut second)
        .await
        .expect_err("duplicate sku across parents must be rejected");
    let cv = rejected(err);

    let composed: repo_composition_middle::WriteRejection = cv.clone().into();
    assert!(
        matches!(&composed, repo_composition_middle::WriteRejection::OrderItemsSkuKey(c) if c.attempted.is_none())
    );
    assert_eq!(
        Into::<&'static str>::into(composed.code()),
        "lane_items_sku_key"
    );
    assert!(
        matches!(&cv, LaneParentConstraintViolation::ItemsSkuKey(conflict) if conflict.attempted.is_none())
    );
    assert!(cv.is_unique());
    assert_eq!(cv.constraint_name(), "lane_items_sku_key");

    // The mapped child leaf becomes the domain rejection.
    assert!(matches!(
        LaneOrderRejection::lift(cv.clone()),
        Ok(LaneOrderRejection::DuplicateSku(_))
    ));

    // An unmapped leaf becomes Fatal(Invariant), retaining the original
    // rejection and its child constraint diagnostics in the source chain.
    let failure: Fail<NothingHoisted, errlanes::lanes!(Transient, Fatal)> =
        es_entity::RepoWriteError::<_>::Rejected(cv).lift();
    let Fail::Fatal(fatal) = failure else {
        panic!("expected invariant")
    };
    assert_eq!(fatal.kind, FatalKind::Invariant);
    assert!(
        std::error::Error::source(&fatal)
            .unwrap()
            .is::<LaneParentConstraintViolation>()
    );
    Ok(())
}
