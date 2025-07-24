# Entity Event

In `es-entity` it is assumed that an `Entity` has an associated `EntityEvent` enum that represents all of the state changes (ie. types of events) that can originate from mutations of said entity.
This enum must be serializable and is stores as a `JSON`-blob in the associated `events` table.

```rust
# extern crate es_entity;
# extern crate sqlx;
# extern crate serde;
use serde::{Deserialize, Serialize};
use es_entity::*;

// Entities must always have an associated id type
type UserId = String;

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "UserId")]
pub enum UserEvent {
    // Typically there is a 'first' event that records the initial state of an `Entity`.
    Initialized { id: UserId, name: String },
    // Every mutation should result in an `Event` that represents the 
    // change that happened.
    // This event represents that the `name` attribute of a user was updated.
    NameUpdated { name: String },
}
```
