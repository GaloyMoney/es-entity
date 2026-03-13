use super::ContextData;

// ── Postgres implementation ──────────────────────────────────────────────

#[cfg(feature = "postgres")]
mod pg {
    use sqlx::{
        Postgres,
        postgres::{PgArgumentBuffer, PgHasArrayType, PgTypeInfo, PgValueRef},
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
        ) -> Result<sqlx::encode::IsNull, Box<dyn std::error::Error + Send + Sync + 'static>>
        {
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

    impl PgHasArrayType for ContextData {
        fn array_type_info() -> PgTypeInfo {
            <serde_json::Value as sqlx::postgres::PgHasArrayType>::array_type_info()
        }
    }
}

// ── SQLite implementation ────────────────────────────────────────────────

#[cfg(feature = "sqlite")]
mod sqlite {
    use sqlx::{
        Sqlite,
        sqlite::{SqliteTypeInfo, SqliteValueRef},
    };

    use super::ContextData;

    impl sqlx::Type<Sqlite> for ContextData {
        fn type_info() -> SqliteTypeInfo {
            <String as sqlx::Type<Sqlite>>::type_info()
        }
    }

    impl<'q> sqlx::Encode<'q, Sqlite> for ContextData {
        fn encode_by_ref(
            &self,
            buf: &mut Vec<sqlx::sqlite::SqliteArgumentValue<'q>>,
        ) -> Result<sqlx::encode::IsNull, Box<dyn std::error::Error + Send + Sync + 'static>>
        {
            let json_str = serde_json::to_string(self)?;
            <String as sqlx::Encode<Sqlite>>::encode(json_str, buf)
        }
    }

    impl<'r> sqlx::Decode<'r, Sqlite> for ContextData {
        fn decode(
            value: SqliteValueRef<'r>,
        ) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
            let json_str = <String as sqlx::Decode<Sqlite>>::decode(value)?;
            let res: ContextData = serde_json::from_str(&json_str)?;
            Ok(res)
        }
    }
}
