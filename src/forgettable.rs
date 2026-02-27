//! Support for forgettable event data (e.g., for GDPR compliance).
//!
//! The [`Forgettable<T>`] wrapper marks event fields containing personal data that
//! can be permanently deleted. Sensitive field values are stored in a separate
//! "forgettable payloads" table. Calling `forget()` on the repository deletes
//! those payloads, leaving the events intact but with `null` for forgotten fields.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Wrapper for event fields containing data that can be forgotten (e.g., for GDPR).
///
/// When present, holds the value via `Set(T)`. After `forget()` is called on the
/// entity's repository, the value is permanently deleted and this becomes `Forgotten`.
///
/// # Serde Behavior
///
/// - `Set(value)` serializes as the inner value (transparent)
/// - `Forgotten` serializes as `null`
/// - Deserializing `null` produces `Forgotten`, non-null produces `Set(value)`
///
/// # Example
///
/// ```rust
/// use es_entity::Forgettable;
///
/// let name: Forgettable<String> = Forgettable::Set("Alice".to_string());
/// assert_eq!(name.value(), Some(&"Alice".to_string()));
///
/// let forgotten: Forgettable<String> = Forgettable::Forgotten;
/// assert_eq!(forgotten.value(), None);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Forgettable<T> {
    /// The value is present (not yet forgotten).
    Set(T),
    /// The value has been permanently deleted.
    Forgotten,
}

impl<T> Forgettable<T> {
    /// Returns a reference to the inner value, or `None` if forgotten.
    pub fn value(&self) -> Option<&T> {
        match self {
            Forgettable::Set(v) => Some(v),
            Forgettable::Forgotten => None,
        }
    }

    /// Consumes self and returns the inner value, or `None` if forgotten.
    pub fn into_value(self) -> Option<T> {
        match self {
            Forgettable::Set(v) => Some(v),
            Forgettable::Forgotten => None,
        }
    }

    /// Returns `true` if the value is present.
    pub fn is_set(&self) -> bool {
        matches!(self, Forgettable::Set(_))
    }

    /// Returns `true` if the value has been forgotten.
    pub fn is_forgotten(&self) -> bool {
        matches!(self, Forgettable::Forgotten)
    }
}

impl<T: Serialize> Serialize for Forgettable<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Forgettable::Set(value) => value.serialize(serializer),
            Forgettable::Forgotten => serializer.serialize_none(),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Forgettable<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Option::<T>::deserialize(deserializer)?;
        match value {
            Some(v) => Ok(Forgettable::Set(v)),
            None => Ok(Forgettable::Forgotten),
        }
    }
}

/// Extracts forgettable field values from an event JSON object, replacing them with `null`.
///
/// Returns `Some(payload)` containing the extracted values if any non-null forgettable
/// fields were found, or `None` if all forgettable fields were already null.
#[doc(hidden)]
pub fn extract_forgettable_payload(
    event_json: &mut serde_json::Value,
    field_names: &[&str],
) -> Option<serde_json::Value> {
    if field_names.is_empty() {
        return None;
    }
    let obj = event_json.as_object_mut()?;
    let mut payload = serde_json::Map::new();
    let mut any_extracted = false;
    for &name in field_names {
        if let Some(value) = obj.get(name)
            && !value.is_null()
        {
            payload.insert(name.to_string(), value.clone());
            obj.insert(name.to_string(), serde_json::Value::Null);
            any_extracted = true;
        }
    }
    if any_extracted {
        Some(serde_json::Value::Object(payload))
    } else {
        None
    }
}

/// Injects forgettable payload values back into an event JSON object.
///
/// Merges all keys from the payload into the event JSON, overwriting `null` values
/// with the original data.
#[doc(hidden)]
pub fn inject_forgettable_payload(event_json: &mut serde_json::Value, payload: &serde_json::Value) {
    if let (Some(event_obj), Some(payload_obj)) = (event_json.as_object_mut(), payload.as_object())
    {
        for (key, value) in payload_obj {
            event_obj.insert(key.clone(), value.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_set() {
        let value: Forgettable<String> = Forgettable::Set("Alice".to_string());
        let json = serde_json::to_value(&value).unwrap();
        assert_eq!(json, serde_json::json!("Alice"));
    }

    #[test]
    fn serialize_forgotten() {
        let value: Forgettable<String> = Forgettable::Forgotten;
        let json = serde_json::to_value(&value).unwrap();
        assert_eq!(json, serde_json::json!(null));
    }

    #[test]
    fn deserialize_value() {
        let json = serde_json::json!("Alice");
        let value: Forgettable<String> = serde_json::from_value(json).unwrap();
        assert_eq!(value, Forgettable::Set("Alice".to_string()));
    }

    #[test]
    fn deserialize_null() {
        let json = serde_json::json!(null);
        let value: Forgettable<String> = serde_json::from_value(json).unwrap();
        assert_eq!(value, Forgettable::Forgotten);
    }

    #[test]
    fn roundtrip_in_struct() {
        #[derive(Serialize, Deserialize, Debug, PartialEq)]
        struct Event {
            #[serde(rename = "type")]
            kind: String,
            name: Forgettable<String>,
            email: String,
        }

        let event = Event {
            kind: "initialized".to_string(),
            name: Forgettable::Set("Alice".to_string()),
            email: "alice@test.com".to_string(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["name"], serde_json::json!("Alice"));

        let deserialized: Event = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, event);

        // With forgotten
        let event_forgotten = Event {
            kind: "initialized".to_string(),
            name: Forgettable::Forgotten,
            email: "alice@test.com".to_string(),
        };
        let json = serde_json::to_value(&event_forgotten).unwrap();
        assert_eq!(json["name"], serde_json::json!(null));

        let deserialized: Event = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, event_forgotten);
    }

    #[test]
    fn extract_payload() {
        let mut json = serde_json::json!({
            "type": "initialized",
            "id": "uuid",
            "name": "Alice",
            "email": "alice@test.com"
        });

        let payload = extract_forgettable_payload(&mut json, &["name"]);

        assert_eq!(json["name"], serde_json::json!(null));
        assert_eq!(json["email"], serde_json::json!("alice@test.com"));

        let payload = payload.unwrap();
        assert_eq!(payload["name"], serde_json::json!("Alice"));
    }

    #[test]
    fn extract_payload_already_null() {
        let mut json = serde_json::json!({
            "type": "initialized",
            "name": null
        });

        let payload = extract_forgettable_payload(&mut json, &["name"]);
        assert!(payload.is_none());
    }

    #[test]
    fn inject_payload() {
        let mut json = serde_json::json!({
            "type": "initialized",
            "id": "uuid",
            "name": null,
            "email": "alice@test.com"
        });

        let payload = serde_json::json!({"name": "Alice"});
        inject_forgettable_payload(&mut json, &payload);

        assert_eq!(json["name"], serde_json::json!("Alice"));
        assert_eq!(json["email"], serde_json::json!("alice@test.com"));
    }

    #[test]
    fn value_helpers() {
        let set: Forgettable<String> = Forgettable::Set("test".to_string());
        assert!(set.is_set());
        assert!(!set.is_forgotten());
        assert_eq!(set.value(), Some(&"test".to_string()));

        let forgotten: Forgettable<String> = Forgettable::Forgotten;
        assert!(!forgotten.is_set());
        assert!(forgotten.is_forgotten());
        assert_eq!(forgotten.value(), None);
    }
}
