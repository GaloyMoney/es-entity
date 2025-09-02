use sqlx::{
    Postgres,
    postgres::{PgArgumentBuffer, PgTypeInfo, PgValueRef},
};

use super::ContextData;

impl sqlx::Type<Postgres> for ContextData {
    fn type_info() -> PgTypeInfo {
        <serde_json::Value as sqlx::Type<Postgres>>::type_info()
    }
}

impl<'q> sqlx::Encode<'q, Postgres> for ContextData {
    fn encode_by_ref(
        &self,
        buf: &mut PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, Box<dyn std::error::Error + Send + Sync + 'static>> {
        let json_value = serde_json::to_value(&self.0)?;
        <serde_json::Value as sqlx::Encode<Postgres>>::encode_by_ref(&json_value, buf)
    }
}

impl<'r> sqlx::Decode<'r, Postgres> for ContextData {
    fn decode(
        value: PgValueRef<'r>,
    ) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
        let json_value = <serde_json::Value as sqlx::Decode<Postgres>>::decode(value)?;
        let res: ContextData = serde_json::from_value(json_value)?;
        Ok(res)
    }
}
