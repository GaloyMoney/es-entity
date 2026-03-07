mod entities;
mod helpers;

use entities::profile::*;
use es_entity::*;
use sqlx::PgPool;

/// Profiles repo with custom accessors:
/// - `name`: field-path accessor (`data.name`) — accesses nested struct field
/// - `display_name`: method-call accessor (`display_name()`) — returns owned String
/// - `email`: direct field access — no custom accessor
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "Profile",
    columns(
        name(ty = "String", update(accessor = "data.name")),
        display_name(
            ty = "String",
            create(accessor = "display_name()"),
            update(accessor = "display_name()")
        ),
        email(ty = "String"),
    )
)]
pub struct Profiles {
    pool: PgPool,
}

impl Profiles {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[tokio::test]
async fn update_all_with_custom_accessors() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let profiles = Profiles::new(pool);

    let alice_email = format!("alice_{}@test.com", ProfileId::new());
    let bob_email = format!("bob_{}@test.com", ProfileId::new());
    let bob_new_email = format!("bob_new_{}@test.com", ProfileId::new());

    let new_profiles = vec![
        NewProfile::builder()
            .id(ProfileId::new())
            .name("Alice")
            .email(&alice_email)
            .build()
            .unwrap(),
        NewProfile::builder()
            .id(ProfileId::new())
            .name("Bob")
            .email(&bob_email)
            .build()
            .unwrap(),
    ];

    let mut created = profiles.create_all(new_profiles).await?;
    assert_eq!(created.len(), 2);

    let _ = created[0].update_name("Alice_updated");
    let _ = created[1].update_email(bob_new_email.clone());

    let n_events = profiles.update_all(&mut created).await?;
    assert_eq!(n_events, 2);

    let loaded_alice = profiles.find_by_id(created[0].id).await?;
    assert_eq!(loaded_alice.data.name, "Alice_updated");
    assert_eq!(loaded_alice.email, alice_email);

    let loaded_bob = profiles.find_by_id(created[1].id).await?;
    assert_eq!(loaded_bob.data.name, "Bob");
    assert_eq!(loaded_bob.email, bob_new_email);

    Ok(())
}
