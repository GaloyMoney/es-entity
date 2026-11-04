#![cfg(feature = "graphql")]

use async_graphql::{
    EmptyMutation, EmptySubscription, InputType, Object, SDLExportOptions, Schema, Value,
};

es_entity::entity_id! { CustomerId }

struct Query;

#[Object]
impl Query {
    async fn customer_id(&self, id: CustomerId) -> CustomerId {
        id
    }

    async fn uuid(&self, id: es_entity::graphql::UUID) -> es_entity::graphql::UUID {
        id
    }
}

#[tokio::test]
async fn uuid_metadata_is_published_for_entity_ids_and_the_uuid_wrapper() {
    let schema = Schema::new(Query, EmptyMutation, EmptySubscription);
    let sdl = schema.sdl_with_options(SDLExportOptions::new().include_specified_by());
    for name in ["CustomerId", "UUID"] {
        assert!(sdl.contains(&format!(
            "scalar {name} @specifiedBy(url: \"https://www.rfc-editor.org/rfc/rfc9562\")"
        )));
        let response = schema
            .execute(format!(
                "{{ __type(name: \"{name}\") {{ name specifiedByURL }} }}"
            ))
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json().unwrap();
        assert_eq!(data["__type"]["name"], name);
        assert_eq!(
            data["__type"]["specifiedByURL"],
            "https://www.rfc-editor.org/rfc/rfc9562"
        );
    }
}

#[test]
fn uuid_metadata_preserves_entity_id_coercion() {
    for id in [
        "57a5815a-a257-45d8-8687-d9ecb9687ec8",
        "01a1a229-e847-72a6-bf1d-314c8ea6387d",
        "01A1A229-E847-72A6-BF1D-314C8EA6387D",
    ] {
        assert!(<CustomerId as InputType>::parse(Some(Value::String(id.to_owned()))).is_ok());
    }
    assert!(<CustomerId as InputType>::parse(Some(Value::String("30533".to_owned()))).is_err());
}
