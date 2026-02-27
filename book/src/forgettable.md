# Forgettable Data

In event sourcing, events are immutable — they form the permanent audit log. But regulations like GDPR require the ability to permanently delete personal data on request.

`es-entity` solves this with the `Forgettable<T>` wrapper type. Fields marked as `Forgettable` have their values stored separately from the event data, so they can be deleted independently without rewriting event history.

## How It Works

1. When an event is persisted, any `Forgettable<T>` field values are **extracted** and stored in a separate `_forgettable_payloads` table
2. The event itself is stored with `null` in place of those fields
3. When loading, the payload values are **injected** back into the event before deserialization
4. Calling `forget()` deletes the payloads — events remain intact but with `null` for forgotten fields

## Database Setup

You need one additional table alongside your events table:

```sql
CREATE TABLE customers (
  id UUID PRIMARY KEY,
  name VARCHAR NOT NULL,
  email VARCHAR UNIQUE NOT NULL,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE customer_events (
  id UUID NOT NULL REFERENCES customers(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

-- The forgettable payloads table
CREATE TABLE customer_forgettable_payloads (
  entity_id UUID NOT NULL REFERENCES customers(id),
  sequence INT NOT NULL,
  payload JSONB NOT NULL,
  UNIQUE(entity_id, sequence)
);
```

## Defining Forgettable Fields

Wrap any personal data fields in `Forgettable<T>` in your event enum:

```rust
# extern crate es_entity;
# extern crate sqlx;
# extern crate serde;
# use serde::{Deserialize, Serialize};
use es_entity::*;

es_entity::entity_id! { CustomerId }

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "CustomerId")]
pub enum CustomerEvent {
    Initialized {
        id: CustomerId,
        // `name` is personal data — wrap it in Forgettable
        name: Forgettable<String>,
        // `email` is NOT forgettable — it stays in the event
        email: String,
    },
    NameUpdated {
        name: Forgettable<String>,
    },
    EmailUpdated {
        email: String,
    },
}
# fn main() {}
```

The `EsEvent` derive macro detects `Forgettable` fields and generates the extraction/injection code automatically.

## Accessing Forgettable Values

`Forgettable<T>` is an opaque type. You cannot pattern-match the inner value directly. Instead, use `.value()` which returns an `Option<ForgettableRef<T>>`:

```rust
# extern crate es_entity;
use es_entity::Forgettable;

let name: Forgettable<String> = Forgettable::new("Alice".to_string());

// .value() returns Option<ForgettableRef<T>>
// ForgettableRef derefs to T but does NOT implement Serialize
if let Some(val) = name.value() {
    assert_eq!(&*val, "Alice");
}

let forgotten: Forgettable<String> = Forgettable::forgotten();
assert!(forgotten.value().is_none());
# fn main() {}
```

`ForgettableRef` intentionally does **not** implement `Serialize`, preventing accidental re-serialization of personal data into secondary stores.

## Hydrating Entities

In `TryFromEvents`, use `.value()` to read forgettable fields and provide a fallback for forgotten values:

```rust
# extern crate es_entity;
# extern crate sqlx;
# extern crate serde;
# extern crate derive_builder;
# use serde::{Deserialize, Serialize};
# use derive_builder::Builder;
# use es_entity::*;
# es_entity::entity_id! { CustomerId }
# #[derive(EsEvent, Debug, Serialize, Deserialize)]
# #[serde(tag = "type", rename_all = "snake_case")]
# #[es_event(id = "CustomerId")]
# pub enum CustomerEvent {
#     Initialized { id: CustomerId, name: Forgettable<String>, email: String },
#     NameUpdated { name: Forgettable<String> },
#     EmailUpdated { email: String },
# }
#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EsEntityError"))]
pub struct Customer {
    pub id: CustomerId,
    pub name: String,
    pub email: String,
    events: EntityEvents<CustomerEvent>,
}

impl TryFromEvents<CustomerEvent> for Customer {
    fn try_from_events(events: EntityEvents<CustomerEvent>) -> Result<Self, EsEntityError> {
        let mut builder = CustomerBuilder::default();
        for event in events.iter_all() {
            match event {
                CustomerEvent::Initialized { id, name, email } => {
                    builder = builder
                        .id(*id)
                        // Provide a fallback for forgotten values
                        .name(
                            name.value()
                                .map(|r| r.clone())
                                .unwrap_or_else(|| "[forgotten]".into()),
                        )
                        .email(email.clone());
                }
                CustomerEvent::NameUpdated { name } => {
                    if let Some(n) = name.value() {
                        builder = builder.name(n.clone());
                    }
                }
                CustomerEvent::EmailUpdated { email } => {
                    builder = builder.email(email.clone());
                }
            }
        }
        builder.events(events).build()
    }
}
# impl IntoEvents<CustomerEvent> for NewCustomer {
#     fn into_events(self) -> EntityEvents<CustomerEvent> {
#         EntityEvents::init(self.id, [CustomerEvent::Initialized {
#             id: self.id, name: Forgettable::new(self.name), email: self.email,
#         }])
#     }
# }
# pub struct NewCustomer { id: CustomerId, name: String, email: String }
# fn main() {}
```

## Repository Configuration

Enable forgettable on the repository with the `forgettable` attribute:

```rust,ignore
#[derive(EsRepo)]
#[es_repo(
    entity = "Customer",
    columns(name = "String", email = "String"),
    forgettable,
)]
pub struct Customers {
    pool: sqlx::PgPool,
}
```

This generates a `forget` method on the repository that:
1. Deletes all forgettable payloads for the entity from the database
2. Rebuilds the entity in-place with forgotten fields set to `Forgettable::forgotten()`

```rust,ignore
// Load the entity
let mut customer = customers.find_by_id(id).await?;
assert_eq!(customer.name, "Alice");

// Forget personal data — updates `customer` in place
customers.forget(&mut customer).await?;

// The entity immediately reflects the forgotten state
assert_eq!(customer.name, "[forgotten]");
```

## Delete and Forgettable

When `forgettable` is enabled and `delete = "soft"` is configured, calling `delete()` will also automatically delete all forgettable payloads for the entity. This prevents orphaned personal data from remaining in the database after a soft delete.

```rust,ignore
#[derive(EsRepo)]
#[es_repo(
    entity = "Customer",
    columns(name = "String", email = "String"),
    delete = "soft",
    forgettable,
)]
pub struct Customers {
    pool: sqlx::PgPool,
}

// Soft-delete also cleans up forgettable payloads
customers.delete(customer).await?;
```
