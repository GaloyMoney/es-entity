mod helpers;
mod user;

use user::*;

#[tokio::test]
async fn create() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;

    let repo = UserRepo::new(pool);

    let new_user = NewUser::builder()
        .id(UserId::new())
        .name("Frank")
        .build()
        .unwrap();

    let user = repo.create(new_user).await?;

    assert_eq!(user.name, "Frank");

    Ok(())
}
