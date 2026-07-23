#![cfg(feature = "json-schema")]
//! Proof that types containing [`Forgettable<T>`] can derive
//! [`schemars::JsonSchema`] without field-level `#[schemars(with = ...)]`
//! workarounds, and that the field schema matches `Option<T>`.

use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use es_entity::*;

es_entity::entity_id! { UserId }

#[derive(EsEvent, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "UserId")]
pub enum UserEvent {
    Initialized {
        id: UserId,
        name: Forgettable<String>,
        email: String,
    },
}

#[test]
fn forgettable_schema_equals_option_schema() {
    assert_eq!(
        schema_for!(Forgettable<String>),
        schema_for!(Option<String>)
    );
}

#[test]
fn event_with_forgettable_derives_json_schema() {
    #[derive(JsonSchema)]
    #[allow(dead_code)]
    struct WithOption {
        name: Option<String>,
    }

    let event_schema = serde_json::to_value(schema_for!(UserEvent)).unwrap();
    let option_schema = serde_json::to_value(schema_for!(WithOption)).unwrap();

    let initialized = &event_schema["oneOf"][0];
    assert_eq!(
        initialized["properties"]["name"],
        option_schema["properties"]["name"]
    );
    // The field must be present in the serialized JSON (as null), so it is required.
    assert!(
        initialized["required"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("name"))
    );
}
