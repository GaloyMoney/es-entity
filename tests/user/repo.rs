use es_entity::*;
use sqlx::PgPool;

use super::{entity::*, error::*};

#[derive(EsRepo, Debug)]
#[es_repo(entity = "User", err = "UserError", columns(name(ty = "String")))]
pub struct Users {
    pool: PgPool,
}

impl Users {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
