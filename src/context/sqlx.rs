use sqlx::postgres::{PgHasArrayType, PgValueRef};

use crate::db;

use super::ContextData;

impl sqlx::Type<db::Db> for ContextData {
    fn type_info() -> db::TypeInfo {
        <serde_json::Value as sqlx::Type<db::Db>>::type_info()
    }
}

impl<'q> sqlx::Encode<'q, db::Db> for ContextData {
    fn encode_by_ref(
        &self,
        buf: &mut db::ArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, Box<dyn std::error::Error + Send + Sync + 'static>> {
        let json_value = serde_json::to_value(&self.0)?;
        <serde_json::Value as sqlx::Encode<db::Db>>::encode_by_ref(&json_value, buf)
    }
}

impl<'r> sqlx::Decode<'r, db::Db> for ContextData {
    fn decode(
        value: PgValueRef<'r>,
    ) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
        let json_value = <serde_json::Value as sqlx::Decode<db::Db>>::decode(value)?;
        let res: ContextData = serde_json::from_value(json_value)?;
        Ok(res)
    }
}

impl PgHasArrayType for ContextData {
    fn array_type_info() -> db::TypeInfo {
        <serde_json::Value as PgHasArrayType>::array_type_info()
    }
}
