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
  -- `name` is indexed as a forgettable column (see below): it must be
  -- nullable so `forget()` can set it to NULL.
  name VARCHAR,
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

// Create with Forgettable::new() or .into()
let name: Forgettable<String> = Forgettable::new("Alice".to_string());
let also_name: Forgettable<String> = "Alice".to_string().into();

// .value() returns Option<ForgettableRef<T>>
// ForgettableRef derefs to T but does NOT implement Serialize
if let Some(val) = name.value() {
    assert_eq!(&*val, "Alice");
}

// Forgettable::forgotten() or Default::default()
let forgotten: Forgettable<String> = Forgettable::forgotten();
let also_forgotten: Forgettable<String> = Default::default();
assert!(forgotten.value().is_none());
# fn main() {}
```

`ForgettableRef` intentionally does **not** implement `Serialize`, preventing accidental re-serialization of personal data into secondary stores.

## Hydrating Entities

Keep forgettable data as `Forgettable<T>` on the entity too, so the entity can
faithfully represent the forgotten state (and so the field can back a
[forgettable index column](#forgettable-index-columns)). Hydration just carries
the event's value through:

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
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
pub struct Customer {
    pub id: CustomerId,
    pub name: Forgettable<String>,
    pub email: String,
    events: EntityEvents<CustomerEvent>,
}

impl TryFromEvents<CustomerEvent> for Customer {
    fn try_from_events(events: EntityEvents<CustomerEvent>) -> Result<Self, EntityHydrationError> {
        let mut builder = CustomerBuilder::default();
        for event in events.iter_all() {
            match event {
                CustomerEvent::Initialized { id, name, email } => {
                    builder = builder
                        .id(*id)
                        // Carry the Forgettable through; it is `Some` while live
                        // and `None` (forgotten) once `forget()` has run.
                        .name(name.clone())
                        .email(email.clone());
                }
                CustomerEvent::NameUpdated { name } => {
                    builder = builder.name(name.clone());
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
    // `name` is a forgettable index column (queryable, NULLed on forget);
    // `email` is a normal column.
    columns(name = "Forgettable<String>", email = "String"),
    forgettable,
)]
pub struct Customers {
    pool: sqlx::PgPool,
}
```

This generates a `forget` method on the repository that:
1. Deletes all forgettable payloads for the entity from the database
2. Sets any [forgettable index columns](#forgettable-index-columns) to `NULL`
3. Rebuilds the entity in-place with forgotten fields set to `Forgettable::forgotten()`

```rust,ignore
// Load the entity
let mut customer = customers.find_by_id(id).await?;
assert_eq!(customer.name.value().map(|v| v.clone()), Some("Alice".to_string()));

// Forget personal data — updates `customer` in place
customers.forget(&mut customer).await?;

// The entity immediately reflects the forgotten state
assert!(customer.name.is_forgotten());
```

## Custom Queries with `es_query!`

If you write custom queries using `es_query!`, you must pass the `forgettable_tbl` parameter so the generated SQL includes the LEFT JOIN for forgettable payloads:

```rust,ignore
let query = es_query!(
    entity = Customer,
    sql = "SELECT * FROM customers WHERE email = $1",
    args = [email as String],
    forgettable_tbl = "customer_forgettable_payloads",
);
```

If you omit `forgettable_tbl` on an event type that has `Forgettable<T>` fields, you get a **compile-time error**:

```text
error: es_query! requires `forgettable_tbl` parameter when the event type has Forgettable<T> fields
```

This prevents silently loading events without their forgettable data.

## Delete and Forgettable

When `forgettable` is enabled and `delete = "soft"` is configured, calling `delete()` also auto-forgets: it deletes all forgettable payloads for the entity **and** sets any forgettable index columns to `NULL` (instead of re-persisting the live value). This prevents orphaned personal data from remaining in the database after a soft delete.

```rust,ignore
#[derive(EsRepo)]
#[es_repo(
    entity = "Customer",
    columns(name = "Forgettable<String>", email = "String"),
    delete = "soft",
    forgettable,
)]
pub struct Customers {
    pool: sqlx::PgPool,
}

// Soft-delete also cleans up forgettable payloads and nulls forgettable columns
customers.delete(customer).await?;
```

## Forgettable Index Columns

`forgettable` scrubs the **event stream** (the payloads table). It does **not**
touch columns you materialise into the lookup table via `columns(...)`. If you
index a field that is also personal data as a plain column, the value survives
`forget()` in that column — a leak. Declaring the column `Forgettable<Inner>`
(as in the configuration above) closes the gap.

A `Forgettable<Inner>` column:

- **Stores a nullable index column** (`Inner` while live, `NULL` once forgotten) —
  the database column must therefore be nullable.
- **Is queried by `Inner`**, exactly like a naked column:
  `find_by_name("Alice")` / `list_by_name(...)` behave the same as for
  `name = "String"`.
- **Is set to `NULL` by `forget()`** — in the same transaction as the payload
  delete and the in-place rebuild.
- **Is set to `NULL` by soft `delete()`** (auto-forget), instead of
  re-persisting the live value.

The types must line up: the **entity** field carries the `Forgettable`, while the
**`New` entity** field holds the plain inner value (a freshly-created entity
always has it set):

```rust,ignore
pub struct Customer {
    pub id: CustomerId,
    pub name: Forgettable<String>, // hydrated field is Forgettable
    // ...
}

pub struct NewCustomer {
    pub id: CustomerId,
    pub name: String,              // New entity holds the raw value
    // ...
}
```

After `forget()`, the rebuilt entity's field is `Forgettable::forgotten()` and
the `name` column in the lookup table is `NULL`, so the value is retained
nowhere the framework materialises it.

**Important:** The payloads are *hard-deleted* even when the entity is only soft-deleted. If the entity is later restored, the forgettable fields will remain permanently forgotten.
