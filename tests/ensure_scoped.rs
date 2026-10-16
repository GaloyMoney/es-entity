mod helpers;

use es_entity::errlanes::Fail;
use es_entity::*;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

es_entity::entity_id! {
    EnsureMemberId,
    TenantId
}

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "EnsureMemberId")]
pub enum EnsureMemberEvent {
    Initialized {
        id: EnsureMemberId,
        tenant_id: TenantId,
        email: String,
    },
}

#[derive(EsEntity)]
pub struct EnsureMember {
    pub id: EnsureMemberId,
    pub tenant_id: TenantId,
    pub email: String,
    events: EntityEvents<EnsureMemberEvent>,
}

impl TryFromEvents<EnsureMemberEvent> for EnsureMember {
    fn try_from_events(
        events: EntityEvents<EnsureMemberEvent>,
    ) -> Result<Self, EntityHydrationError> {
        let (id, tenant_id, email) = events
            .iter_all()
            .map(
                |EnsureMemberEvent::Initialized {
                     id,
                     tenant_id,
                     email,
                 }| (*id, *tenant_id, email.clone()),
            )
            .next()
            .expect("an initialized event");
        Ok(EnsureMember {
            id,
            tenant_id,
            email,
            events,
        })
    }
}

pub struct NewEnsureMember {
    pub id: EnsureMemberId,
    pub tenant_id: TenantId,
    pub email: String,
}

impl IntoEvents<EnsureMemberEvent> for NewEnsureMember {
    type IdSource = FromCaller;

    fn into_events(
        self,
        builder: EntityEventsBuilder<EnsureMemberEvent, FromCaller>,
    ) -> EntityEvents<EnsureMemberEvent> {
        builder.initial_events(
            self.id,
            [EnsureMemberEvent::Initialized {
                id: self.id,
                tenant_id: self.tenant_id,
                email: self.email,
            }],
        )
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "EnsureMember",
    id(source = caller),
    columns(tenant_id(ty = "TenantId", scope), email(ty = "String"))
)]
pub struct EnsureMembers {
    pool: PgPool,
}

fn member(id: EnsureMemberId, tenant_id: TenantId, email: &str) -> NewEnsureMember {
    NewEnsureMember {
        id,
        tenant_id,
        email: email.to_string(),
    }
}

fn unique_email() -> String {
    format!("{}@example.com", EnsureMemberId::new())
}

#[tokio::test]
async fn found_within_the_scope_of_new() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureMembers { pool: pool.clone() };
    let tenant = TenantId::new();
    let id = EnsureMemberId::new();
    let email = unique_email();

    let mut op = DbOp::init(&pool).await?;
    let first = repo
        .create_or_find_in_op(&mut op, member(id, tenant, &email))
        .await?;
    let by_email = repo
        .create_or_find_in_op(&mut op, member(EnsureMemberId::new(), tenant, &email))
        .await?;
    op.commit().await?;

    assert!(first.is_created());
    let Ensured::Found(found) = by_email else {
        panic!("the same email in the same tenant must find")
    };
    assert_eq!((found.id, found.tenant_id), (id, tenant));
    Ok(())
}

#[tokio::test]
async fn a_key_held_in_another_scope_is_rejected() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureMembers { pool: pool.clone() };
    let id = EnsureMemberId::new();
    let email = unique_email();

    let mut op = DbOp::init(&pool).await?;
    repo.create_in_op(&mut op, member(id, TenantId::new(), &email))
        .await?;
    let other_tenant = TenantId::new();
    let by_email = repo
        .create_or_find_in_op(&mut op, member(EnsureMemberId::new(), other_tenant, &email))
        .await;
    let by_id = repo
        .create_or_find_in_op(&mut op, member(id, other_tenant, &unique_email()))
        .await;
    op.commit().await?;

    let Err(Fail::Rejected(rejection)) = by_email else {
        panic!("an email held in another tenant must reject")
    };
    let code: &'static str = rejection.code().into();
    assert_eq!(code, "ensure_members_keys_held_outside_scope");
    let EnsureMemberEnsureRejection::KeysHeldOutsideScope { keys } = rejection else {
        panic!("expected KeysHeldOutsideScope")
    };
    assert_eq!(keys, ["email"]);
    assert!(matches!(
        by_id,
        Err(Fail::Rejected(EnsureMemberEnsureRejection::KeysHeldOutsideScope { keys })) if keys == ["id"]
    ));
    Ok(())
}

#[tokio::test]
async fn a_match_outside_the_scope_wins_over_one_inside() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureMembers { pool: pool.clone() };
    let tenant = TenantId::new();
    let own_id = EnsureMemberId::new();
    let foreign_email = unique_email();

    let mut op = DbOp::init(&pool).await?;
    repo.create_in_op(&mut op, member(own_id, tenant, &unique_email()))
        .await?;
    repo.create_in_op(
        &mut op,
        member(EnsureMemberId::new(), TenantId::new(), &foreign_email),
    )
    .await?;
    let res = repo
        .create_or_find_in_op(&mut op, member(own_id, tenant, &foreign_email))
        .await;
    op.commit().await?;

    assert!(matches!(
        res,
        Err(Fail::Rejected(EnsureMemberEnsureRejection::KeysHeldOutsideScope { keys })) if keys == ["email"]
    ));
    Ok(())
}
