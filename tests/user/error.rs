use thiserror::Error;

#[derive(Error, Debug)]
pub enum UserError {
    #[error("UserError - Sqlx: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("UserError - EsEntityError: {0}")]
    EsEntityError(es_entity::EsEntityError),
    #[error("UserError - CursorDestructureError: {0}")]
    CursorDestructureError(#[from] es_entity::CursorDestructureError),
}
es_entity::from_es_entity_error!(UserError);
