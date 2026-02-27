//! Support for forgettable event data (e.g., for GDPR compliance).
//!
//! The [`Forgettable<T>`] wrapper marks event fields containing personal data that
//! can be permanently deleted. Sensitive field values are stored in a separate
//! "forgettable payloads" table. Calling `forget()` on the repository deletes
//! those payloads, leaving the events intact but with `null` for forgotten fields.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use std::{fmt, hash, ops::Deref};

/// Wrapper for event fields containing data that can be forgotten (e.g., for GDPR).
///
/// This is an opaque struct — internal state is private so callers cannot
/// pattern-match to extract the raw value. Use [`Forgettable::value()`] to get
/// a [`ForgettableRef`] that derefs to `T` but does **not** implement `Serialize`,
/// preventing accidental re-serialization of personal data.
///
/// # Serde Behavior
///
/// - **Both** set and forgotten values serialize as `null` to prevent data
///   leakage when events are serialized to secondary stores.
/// - Deserializing `null` produces a forgotten value, non-null produces a set value.
/// - Real values are extracted via [`__extract_payload_value`] **before** serde runs,
///   and stored in the forgettable payloads table.
///
/// # Example
///
/// ```rust
/// use es_entity::Forgettable;
///
/// let name: Forgettable<String> = Forgettable::new("Alice".to_string());
/// assert_eq!(&*name.value().unwrap(), "Alice");
///
/// let forgotten: Forgettable<String> = Forgettable::forgotten();
/// assert!(forgotten.value().is_none());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Forgettable<T>(Option<T>);

impl<T> Default for Forgettable<T> {
    /// Returns a forgotten (empty) `Forgettable`.
    fn default() -> Self {
        Forgettable(None)
    }
}

impl<T> From<T> for Forgettable<T> {
    fn from(value: T) -> Self {
        Forgettable(Some(value))
    }
}

impl<T> Forgettable<T> {
    /// Creates a new `Forgettable` containing the given value.
    pub fn new(value: T) -> Self {
        Forgettable(Some(value))
    }

    /// Creates a forgotten (empty) `Forgettable`.
    pub fn forgotten() -> Self {
        Forgettable(None)
    }

    /// Returns a [`ForgettableRef`] wrapping the inner value, or `None` if forgotten.
    ///
    /// `ForgettableRef` implements `Deref<Target = T>` but **not** `Serialize`,
    /// so you can read the value but cannot accidentally serialize it.
    pub fn value(&self) -> Option<ForgettableRef<'_, T>> {
        self.0.as_ref().map(ForgettableRef)
    }

    /// Returns `true` if the value is present.
    pub fn is_set(&self) -> bool {
        self.0.is_some()
    }

    /// Returns `true` if the value has been forgotten.
    pub fn is_forgotten(&self) -> bool {
        self.0.is_none()
    }
}

impl<T: Serialize> Forgettable<T> {
    /// Extracts the inner value as a `serde_json::Value` for storage in
    /// the forgettable payloads table. Returns `None` if forgotten.
    #[doc(hidden)]
    pub fn __extract_payload_value(&self) -> Option<serde_json::Value> {
        self.0
            .as_ref()
            .map(|v| serde_json::to_value(v).expect("Failed to serialize forgettable field"))
    }
}

impl<T: Serialize> Serialize for Forgettable<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_none()
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Forgettable<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Option::<T>::deserialize(deserializer)?;
        match value {
            Some(v) => Ok(Forgettable(Some(v))),
            None => Ok(Forgettable(None)),
        }
    }
}

/// A non-serializable reference to the value inside a [`Forgettable<T>`].
///
/// Implements `Deref<Target = T>` so you can use it like `&T`, but does **not**
/// implement `Serialize` or `Clone`, preventing accidental re-serialization or
/// extraction of personal data.
pub struct ForgettableRef<'a, T>(&'a T);

impl<T: fmt::Debug> fmt::Debug for ForgettableRef<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<T: fmt::Display> fmt::Display for ForgettableRef<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<T> Deref for ForgettableRef<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.0
    }
}

impl<T: PartialEq> PartialEq<T> for ForgettableRef<'_, T> {
    fn eq(&self, other: &T) -> bool {
        self.0 == other
    }
}

impl<T: PartialEq> PartialEq for ForgettableRef<'_, T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<T: Eq> Eq for ForgettableRef<'_, T> {}

impl<T: hash::Hash> hash::Hash for ForgettableRef<'_, T> {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

/// Injects forgettable payload values back into an event JSON object.
///
/// Merges all keys from the payload into the event JSON, overwriting `null` values
/// with the original data.
#[doc(hidden)]
pub fn inject_forgettable_payload(event_json: &mut serde_json::Value, payload: serde_json::Value) {
    if let (Some(event_obj), serde_json::Value::Object(payload_obj)) =
        (event_json.as_object_mut(), payload)
    {
        for (key, value) in payload_obj {
            event_obj.insert(key, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_set_emits_null() {
        let value: Forgettable<String> = Forgettable::new("Alice".to_string());
        let json = serde_json::to_value(&value).unwrap();
        assert_eq!(json, serde_json::json!(null));
    }

    #[test]
    fn serialize_forgotten_emits_null() {
        let value: Forgettable<String> = Forgettable::forgotten();
        let json = serde_json::to_value(&value).unwrap();
        assert_eq!(json, serde_json::json!(null));
    }

    #[test]
    fn deserialize_value() {
        let json = serde_json::json!("Alice");
        let value: Forgettable<String> = serde_json::from_value(json).unwrap();
        assert_eq!(value, Forgettable::new("Alice".to_string()));
    }

    #[test]
    fn deserialize_null() {
        let json = serde_json::json!(null);
        let value: Forgettable<String> = serde_json::from_value(json).unwrap();
        assert_eq!(value, Forgettable::forgotten());
    }

    #[test]
    fn serialize_struct_with_forgettable_emits_null() {
        #[derive(Serialize, Deserialize, Debug, PartialEq)]
        struct Event {
            #[serde(rename = "type")]
            kind: String,
            name: Forgettable<String>,
            email: String,
        }

        let event = Event {
            kind: "initialized".to_string(),
            name: Forgettable::new("Alice".to_string()),
            email: "alice@test.com".to_string(),
        };
        let json = serde_json::to_value(&event).unwrap();
        // Set serializes as null to prevent data leakage
        assert_eq!(json["name"], serde_json::json!(null));
        assert_eq!(json["email"], serde_json::json!("alice@test.com"));

        // Deserializing null yields Forgotten (real values come from payload table)
        let deserialized: Event = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.name, Forgettable::forgotten());

        // Forgotten also serializes as null
        let event_forgotten = Event {
            kind: "initialized".to_string(),
            name: Forgettable::forgotten(),
            email: "alice@test.com".to_string(),
        };
        let json = serde_json::to_value(&event_forgotten).unwrap();
        assert_eq!(json["name"], serde_json::json!(null));

        let deserialized: Event = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, event_forgotten);
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
        inject_forgettable_payload(&mut json, payload);

        assert_eq!(json["name"], serde_json::json!("Alice"));
        assert_eq!(json["email"], serde_json::json!("alice@test.com"));
    }

    #[test]
    fn value_helpers() {
        let set: Forgettable<String> = Forgettable::new("test".to_string());
        assert!(set.is_set());
        assert!(!set.is_forgotten());
        assert_eq!(&*set.value().unwrap(), "test");

        let forgotten: Forgettable<String> = Forgettable::forgotten();
        assert!(!forgotten.is_set());
        assert!(forgotten.is_forgotten());
        assert!(forgotten.value().is_none());
    }

    #[test]
    fn extract_payload_value() {
        let set: Forgettable<String> = Forgettable::new("Alice".to_string());
        assert_eq!(
            set.__extract_payload_value(),
            Some(serde_json::json!("Alice"))
        );

        let forgotten: Forgettable<String> = Forgettable::forgotten();
        assert_eq!(forgotten.__extract_payload_value(), None);
    }

    #[test]
    fn forgettable_ref_deref() {
        let f = Forgettable::new("hello".to_string());
        let r = f.value().unwrap();
        // Deref to &String
        assert_eq!(r.len(), 5);
        assert_eq!(&*r, "hello");
    }

    #[test]
    fn forgettable_ref_display() {
        let f = Forgettable::new("Alice".to_string());
        let r = f.value().unwrap();
        assert_eq!(format!("{r}"), "Alice");
    }

    #[test]
    fn forgettable_ref_partial_eq() {
        let f = Forgettable::new("Alice".to_string());
        let r = f.value().unwrap();
        assert_eq!(r, "Alice".to_string());
    }

    #[test]
    fn default_is_forgotten() {
        let f: Forgettable<String> = Default::default();
        assert!(f.is_forgotten());
        assert!(f.value().is_none());
    }

    #[test]
    fn from_value() {
        let f: Forgettable<String> = "Alice".to_string().into();
        assert!(f.is_set());
        assert_eq!(&*f.value().unwrap(), "Alice");
    }
}
